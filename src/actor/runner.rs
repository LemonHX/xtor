use std::{
    collections::HashMap,
    sync::{atomic::AtomicU64, Arc},
};

use anyhow::Result;
use futures::{
    channel::{
        mpsc::{self, UnboundedSender},
        oneshot,
    },
    FutureExt, StreamExt,
};
use lazy_static::lazy_static;
use tokio::{sync::RwLock, task::JoinHandle};

use super::{
    addr::{Addr, Event, WeakAddr},
    context::Context,
    supervisor::{Supervise, Supervisor},
};

pub(crate) static ACTOR_ID: AtomicU64 = AtomicU64::new(0);
pub(crate) static RUNNING_ACTOR_COUNTER: AtomicU64 = AtomicU64::new(0);

lazy_static! {
    pub static ref ACTOR_ID_NAME: RwLock<HashMap<u64, Option<String>>> =
        RwLock::new(HashMap::new());
    pub static ref ACTOR_ID_HANDLE: RwLock<HashMap<u64, JoinHandle<Result<()>>>> =
        RwLock::new(HashMap::new());
}

pub type ActorID = u64;

#[async_trait::async_trait]
pub trait Actor: Send + Sync + 'static {
    /// hook for actor initialization
    async fn on_start(&self, _ctx: &Context) -> Result<()> {
        Ok(())
    }
    /// hook for actor shutdown
    async fn on_stop(&self, _ctx: &Context) {}
    /// check the name of the actor
    async fn get_name(&self, ctx: &Context) -> Option<String> {
        ACTOR_ID_NAME.read().await[&ctx.id].clone()
    }
    /// starting an actor
    /// if you want a supervised actor you need to send message to supervisor instead starting it from here
    /// `?Sized` actor is not supported
    async fn spawn(self) -> Result<Addr>
    where
        Self: Sized,
    {
        ActorRunner::new().run(self).await
    }

    async fn spawn_supervised<S: Supervisor>(self, supervisor: &Addr) -> Result<Addr>
    where
        Self: Sized + ActorRestart,
    {
        let addr = ActorRunner::new().supervised_run(self).await?;
        supervisor
            .call::<S, Supervise>(Supervise(addr.clone()))
            .await?;
        Ok(addr)
    }
}

pub struct ActorRunner {
    pub ctx: Context,
    tx: Arc<UnboundedSender<Event>>,
    rx: mpsc::UnboundedReceiver<Event>,
    tx_exit: oneshot::Sender<()>,
}

impl Default for ActorRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl ActorRunner {
    pub fn new() -> Self {
        let (tx_exit, rx_exit) = oneshot::channel::<()>();
        let rx_exit = rx_exit.shared();
        let (ctx, rx, tx) = Context::new(rx_exit);
        Self {
            ctx,
            tx,
            rx,
            tx_exit,
        }
    }
    pub async fn run<A: Actor>(self, actor: A) -> Result<Addr> {
        let Self {
            ctx,
            mut rx,
            tx,
            tx_exit,
        } = self;

        let rx_exit = ctx.rx_exit.clone();
        let id = ctx.id;
        ACTOR_ID_NAME.write().await.insert(id, None);
        let actor = Arc::new(actor);
        let addr = Addr { id, tx, rx_exit };
        ctx.addr.set(addr.downgrade()).unwrap();
        actor.on_start(&ctx).await?;
        let handle = tokio::task::spawn(async move {
            let mut exit_err = Ok(());
            while let Some(event) = rx.next().await {
                match event {
                    Event::Stop(err) => {
                        exit_err = err;
                        break;
                    }
                    Event::Exec(f) => match f(actor.clone(), &ctx).await {
                        Ok(_) => {}
                        Err(err) => {
                            exit_err = Err(err);
                            break;
                        }
                    },
                    Event::Restart => {
                        panic!("this event could only send by supervisor");
                    }
                    Event::AddSupervisor(_) => {
                        panic!("this event could only send by supervisor");
                    }
                }
            }
            actor.on_stop(&ctx).await;
            tx_exit.send(()).unwrap();
            exit_err
        });
        ACTOR_ID_HANDLE.write().await.insert(id, handle);
        Ok(addr)
    }

    pub(crate) async fn supervised_run<A: Actor + ActorRestart>(self, actor: A) -> Result<Addr> {
        let Self {
            ctx,
            mut rx,
            tx,
            tx_exit,
        } = self;

        let rx_exit = ctx.rx_exit.clone();
        let id = ctx.id;
        ACTOR_ID_NAME.write().await.insert(id, None);
        let actor = Arc::new(actor);
        let addr = Addr { id, tx, rx_exit };
        ctx.addr.set(addr.downgrade()).unwrap();
        actor.on_start(&ctx).await?;
        let weakaddr = addr.downgrade();
        let handle = tokio::task::spawn(async move {
            let mut exit_err = Ok(());
            'supervising_loop: loop {
                'event_loop: while let Some(event) = rx.next().await {
                    match event {
                        Event::Stop(err) => {
                            exit_err = err;
                            break 'event_loop;
                        }
                        Event::Exec(f) => match f(actor.clone(), &ctx).await {
                            Ok(_) => {}
                            Err(err) => {
                                exit_err = Err(err);
                                break 'event_loop;
                            }
                        },
                        Event::Restart => {
                            actor.on_restart(&weakaddr).await;
                            continue 'supervising_loop;
                        }
                        Event::AddSupervisor(proxy) => {
                            ctx.supervisors.write().await.push(proxy);
                        }
                    }
                }
                // supervice logic
                if exit_err.is_err() {
                    exit_err = ctx.await_supervisor().await;
                    if exit_err.is_err() {
                        break 'supervising_loop;
                    } else {
                        actor.on_restart(&weakaddr).await;
                        continue 'supervising_loop;
                    }
                } else {
                    break 'supervising_loop;
                }
            }
            actor.on_stop(&ctx).await;
            tx_exit.send(()).unwrap();
            exit_err
        });
        ACTOR_ID_HANDLE.write().await.insert(id, handle);
        Ok(addr)
    }
}

#[async_trait::async_trait]
pub trait ActorRestart {
    async fn on_restart(&self, _addr: &WeakAddr) {}
}