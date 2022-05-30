// An actor called human is a human.
// a message eat could eat a food.
// food has two types: normal and poisoned.
// normal food is good for human.
// poisoned food will kill human.
// normal_hospital is a supervisor that can respawn(lol) a human.

// another thing is called virus.
// virus is contagious.
// virus can infect human.
// virus_hospital will respawn all human if even only one human is dead.
// virus_hospital is not very clever, for every desease it will think that it is
// a virus.

use std::sync::atomic::AtomicBool;

use futures::{join, try_join};
use rand::{prelude::StdRng, Rng, SeedableRng};
use tracing::{error, info, warn};
use xtor::{
    actor::{
        addr::WeakAddr,
        context::Context,
        message::Handler,
        runner::{Actor, ActorRestart},
        supervisor::Supervise,
    },
    utils::default_supervisor::DefaultSupervisor,
};

struct Human(AtomicBool);
impl Default for Human {
    fn default() -> Self {
        Human(AtomicBool::new(false))
    }
}
#[async_trait::async_trait]
impl Actor for Human {}

#[async_trait::async_trait]
impl ActorRestart for Human {
    #[tracing::instrument(
        skip(self),
        fields(addr = addr.get_name_or_id_string().as_str())
    )]
    async fn on_restart(&self, addr: &WeakAddr) {
        warn!("\thospital saved {}", addr.get_name_or_id_string());
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

#[derive(Debug, Clone)]
enum Food {
    Normal(&'static str),
    Poisoned(&'static str),
}

#[derive(Debug)]
#[xtor::message(result = "()")]
struct Eat(Food);

#[async_trait::async_trait]
impl Handler<Eat> for Human {
    #[tracing::instrument(
        skip(self, ctx),
        name = "Human::Eat",
        fields(addr = self.get_name_or_id_string(ctx).as_str())
    )]
    async fn handle(&self, ctx: &Context, msg: Eat) -> anyhow::Result<()> {
        match msg.0 {
            Food::Normal(food) => {
                info!("{} eat {:?}", self.get_name_or_id_string(ctx), food);
                Ok(())
            }
            Food::Poisoned(food) => {
                error!(
                    "{} eat {:?}, and dead",
                    self.get_name_or_id_string(ctx),
                    food
                );
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
                Err(anyhow::anyhow!("dead"))
            }
        }
    }
}

#[derive(Debug)]
#[xtor::message(result = "()")]
struct Virus;

#[async_trait::async_trait]
impl Handler<Virus> for Human {
    #[tracing::instrument(
        skip(self, ctx),
        name = "Human::Virus",
        fields(addr = self.get_name_or_id_string(ctx).as_str())
    )]
    async fn handle(&self, ctx: &Context, _msg: Virus) -> anyhow::Result<()> {
        error!("{} infected, and dead", self.get_name_or_id_string(ctx));
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        Err(anyhow::anyhow!("dead"))
    }
}

#[xtor::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let system_running_duration = 20;

    // create supervisor actor
    let (normal_hospital, virus_hospital) = try_join!(
        DefaultSupervisor::new(
            xtor::utils::default_supervisor::DefaultSupervisorRestartStrategy::OneForOne,
        )
        .spawn(),
        DefaultSupervisor::new(
            xtor::utils::default_supervisor::DefaultSupervisorRestartStrategy::OneForAll,
        )
        .spawn()
    )?;

    // create proxy to supervisor actor
    let (normal_hospital_proxy, virus_hospital_proxy) = join!(
        normal_hospital.proxy::<DefaultSupervisor, Supervise>(),
        virus_hospital.proxy::<DefaultSupervisor, Supervise>()
    );

    // link actors to supervisor
    let alice = Human::default()
        .spawn_supervisable()
        .await?
        .chain_link_to_supervisor(&normal_hospital_proxy)
        .await?
        .chain_link_to_supervisor(&virus_hospital_proxy)
        .await?;
    let bob = Human::default()
        .spawn_supervisable()
        .await?
        .chain_link_to_supervisor(&normal_hospital_proxy)
        .await?;
    // bob said that he has a very strong body
    // thus virus could not come out so he decided to only paying the normal
    // hospital

    let carl = Human::default()
        .spawn_supervisable()
        .await?
        .chain_link_to_supervisor(&normal_hospital_proxy)
        .await?
        .chain_link_to_supervisor(&virus_hospital_proxy)
        .await?;
    let david = Human::default()
        .spawn_supervisable()
        .await?
        .chain_link_to_supervisor(&normal_hospital_proxy)
        .await?
        .chain_link_to_supervisor(&virus_hospital_proxy)
        .await?;

    alice.set_name("Alice").await;
    bob.set_name("Bob").await;
    carl.set_name("Carl").await;
    david.set_name("David").await;

    let humans = [alice.clone(), bob.clone(), carl.clone(), david.clone()];
    let food_feeder = tokio::task::spawn(async move {
        let foods = [
            Food::Normal("apple"),
            Food::Normal("banana"),
            Food::Normal("orange"),
            Food::Normal("pear"),
            Food::Normal("grape"),
            Food::Poisoned("red mushroom"),
        ];
        let start = std::time::Instant::now();
        while start.elapsed().as_secs() < system_running_duration {
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            let mut rng = StdRng::from_entropy();
            let selected_human = humans
                .get(rng.gen_range(0..humans.len()))
                .expect("fail to get human");
            let selected_food = foods
                .get(rng.gen_range(0..foods.len()))
                .expect("fail to get food")
                .clone();
            let _ = selected_human.call::<Human, Eat>(Eat(selected_food)).await;
        }
    });

    let humans = [alice.clone(), bob.clone(), carl.clone(), david.clone()];
    let virus_inflecter = tokio::task::spawn(async move {
        let start = std::time::Instant::now();
        while start.elapsed().as_secs() < system_running_duration {
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            let mut rng = StdRng::from_entropy();
            let selected_human = humans
                .get(rng.gen_range(0..humans.len()))
                .expect("fail to get human");
            let _ = selected_human.call::<Human, Virus>(Virus).await;
        }
    });

    food_feeder.await?;
    virus_inflecter.await?;
    Ok(())
}
