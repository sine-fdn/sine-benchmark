use clap::Parser;
use futures::StreamExt;
use libp2p::{
    gossipsub, identity, noise,
    swarm::{NetworkBehaviour, SwarmEvent},
    upnp, yamux, Multiaddr, PeerId,
};
use log::{error, info};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, error::Error, time::Duration};
use tokio::{
    io::{self, AsyncBufReadExt},
    select,
};

/// Peer-to-peer benchmarking against group average without disclosing inputs
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Session to join, leave empty to start a new session
    #[arg(short, long)]
    address: Option<String>,

    /// Human-readable alias used to identify each participant
    #[arg(short, long)]
    name: String,
}

#[derive(NetworkBehaviour)]
struct MyBehaviour {
    upnp: upnp::tokio::Behaviour,
    gossipsub: gossipsub::Behaviour,
}

#[derive(Serialize, Deserialize)]
enum Msg {
    Join(PeerId, String, Multiaddr),
    Participants(HashMap<PeerId, (String, Multiaddr)>),
    LobbyNowClosed,
}

impl Msg {
    fn serialize(&self) -> Result<Vec<u8>, Box<dyn Error>> {
        Ok(bincode::serialize(&self)?)
    }
}

enum Phase {
    WaitingForParticipants,
    ConfirmingParticipants,
    SendingShares,
    //SendingSums,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init();
    let Args { address, name } = Args::parse();

    let my_key = identity::Keypair::generate_ed25519();
    let my_peer_id = PeerId::from(my_key.public());
    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(my_key.clone())
        .with_tokio()
        .with_tcp(
            Default::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_behaviour(|key| {
            let gossipsub_config = gossipsub::ConfigBuilder::default()
                .heartbeat_interval(Duration::from_secs(10))
                .validation_mode(gossipsub::ValidationMode::Strict)
                .build()
                .map_err(|msg| io::Error::new(io::ErrorKind::Other, msg))?;

            let upnp = upnp::tokio::Behaviour::default();
            let gossipsub = gossipsub::Behaviour::new(
                gossipsub::MessageAuthenticity::Signed(key.clone()),
                gossipsub_config,
            )?;
            Ok(MyBehaviour { upnp, gossipsub })
        })?
        .build();

    let is_leader = address.is_none();
    let topic = gossipsub::IdentTopic::new("lobby");
    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;

    if let Some(addr) = &address {
        let remote: Multiaddr = addr.parse()?;
        swarm.dial(remote)?;
        println!("Joined session at {addr}");
    }

    let mut phase = Phase::WaitingForParticipants;
    let mut stdin = io::BufReader::new(io::stdin()).lines();
    let mut participants = HashMap::<PeerId, (String, Multiaddr)>::new();
    enum Event {
        Upnp(upnp::Event),
        Gossipsub(gossipsub::Event),
        StdIn(String),
    }
    let confirm_msg = "Please double-check the peer ids. Do you want to join the benchmark? [Y/n]";
    let starting_msg = "Starting benchmark with the current participants...";
    loop {
        let ev = select! {
            Ok(Some(line)) = stdin.next_line() => {
                Some(Event::StdIn(line))
            }
            ev = swarm.select_next_some() => match ev {
                SwarmEvent::Behaviour(MyBehaviourEvent::Upnp(ev)) => Some(Event::Upnp(ev)),
                SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(ev)) => Some(Event::Gossipsub(ev)),
                ev => {
                    info!("{ev:?}");
                    None
                }
            },
        };
        let Some(ev) = ev else {
            continue;
        };
        match ev {
            Event::StdIn(line) => match phase {
                Phase::WaitingForParticipants if is_leader => {
                    if participants.len() < 3 {
                        println!("Cannot start yet, at least 3 participants are needed to ensure that inputs remain private.");
                        continue;
                    }
                    println!("{starting_msg}");
                    phase = Phase::SendingShares;
                    let msg = Msg::LobbyNowClosed.serialize()?;
                    swarm
                        .behaviour_mut()
                        .gossipsub
                        .publish(topic.clone(), msg)?;
                }
                Phase::WaitingForParticipants => {}
                Phase::ConfirmingParticipants => {
                    if line.trim().is_empty() || line.trim().to_lowercase() == "y" {
                        println!("{starting_msg}");
                        phase = Phase::SendingShares;
                    } else if line.trim().to_lowercase() == "n" {
                        std::process::exit(0);
                    } else {
                        println!("{confirm_msg}");
                    }
                }
                Phase::SendingShares => {}
            },
            Event::Upnp(upnp::Event::NewExternalAddr(addr)) => {
                if is_leader {
                    println!("A new session has been started, others can join using the following command:");
                    println!("cargo run -- --address={addr} --name=<your_alias>");
                    println!("");
                    println!(
                        "Press ENTER to start the benchmark once all participants have joined."
                    );
                    println!("");
                    println!("-- Participants --");
                    println!("{my_peer_id} - {name}");
                } else {
                    let msg = Msg::Join(my_peer_id, name.clone(), addr.clone()).serialize()?;
                    swarm
                        .behaviour_mut()
                        .gossipsub
                        .publish(topic.clone(), msg)?;
                    println!("");
                    println!("-- Participants --");
                    println!("{my_peer_id} - {name}");
                }
                swarm.behaviour_mut().gossipsub.subscribe(&topic)?;
                participants.insert(my_peer_id, (name.clone(), addr.clone()));
            }
            Event::Upnp(upnp::Event::GatewayNotFound) => {
                println!("Gateway does not support UPnP");
                break;
            }
            Event::Upnp(upnp::Event::NonRoutableGateway) => {
                println!("Gateway is not exposed directly to the public Internet, i.e. it itself has a private IP address.");
                break;
            }
            Event::Upnp(ev) => info!("{ev:?}"),
            Event::Gossipsub(gossipsub::Event::Message {
                propagation_source,
                message,
                ..
            }) => {
                let Ok(msg) = bincode::deserialize::<Msg>(&message.data) else {
                    error!("Received invalid message from {propagation_source}");
                    continue;
                };
                match msg {
                    Msg::Join(peer_id, name, addr) if is_leader => {
                        println!("{peer_id} - {name}");
                        participants.insert(peer_id, (name, addr));
                        let msg = Msg::Participants(participants.clone()).serialize()?;
                        if let Err(e) = swarm.behaviour_mut().gossipsub.publish(topic.clone(), msg)
                        {
                            error!("Could not publish to gossipsub: {e:?}");
                        }
                    }
                    Msg::Join(_, _, _) => {}
                    Msg::Participants(all_participants) => {
                        for (peer_id, (name, _)) in all_participants.iter() {
                            if !participants.contains_key(&peer_id) {
                                println!("{peer_id} - {name}");
                            }
                        }
                        participants = all_participants;
                    }
                    Msg::LobbyNowClosed if !is_leader => {
                        if participants.len() < 3 {
                            eprintln!("Someone tried to start a benchmark with < 3 participants!");
                            std::process::exit(1);
                        }
                        phase = Phase::ConfirmingParticipants;
                        println!("");
                        println!("{confirm_msg}");
                    }
                    Msg::LobbyNowClosed => {
                        error!("This message should never be sent to the benchmark leader!");
                    }
                }
            }
            Event::Gossipsub(ev) => info!("{ev:?}"),
        }
    }
    Ok(())
}
