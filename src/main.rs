use clap::Parser;
use futures::StreamExt;
use libp2p::{
    gossipsub, noise,
    swarm::{NetworkBehaviour, SwarmEvent},
    upnp, yamux, Multiaddr,
};
use log::{error, info};
use rsa::signature::SignatureEncoding;
use rsa::signature::Verifier;
use rsa::{pkcs1v15::VerifyingKey, signature::RandomizedSigner};
use rsa::{
    pkcs1v15::{Signature, SigningKey},
    pkcs8::{DecodePublicKey, EncodePublicKey, LineEnding},
    sha2::Sha256,
    Pkcs1v15Encrypt, RsaPrivateKey, RsaPublicKey,
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, error::Error, time::Duration};
use tokio::{
    io::{self, AsyncBufReadExt},
    select,
};

const KEY_BITS: usize = 2048;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
struct PublicKey(String);

impl std::fmt::Display for PublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let hash = blake3::hash(self.0.as_bytes());
        let bytes = hash.as_bytes();
        let h1 = u32::from_le_bytes(<[u8; 4]>::try_from(&bytes[0..4]).unwrap());
        let h2 = u32::from_le_bytes(<[u8; 4]>::try_from(&bytes[4..8]).unwrap());
        let h3 = u32::from_le_bytes(<[u8; 4]>::try_from(&bytes[8..12]).unwrap());
        let h4 = u32::from_le_bytes(<[u8; 4]>::try_from(&bytes[12..16]).unwrap());
        write!(f, "{:08x} {:08x} {:08x} {:08x}", h1, h2, h3, h4)
    }
}

impl From<RsaPublicKey> for PublicKey {
    fn from(key: RsaPublicKey) -> Self {
        Self(
            key.to_public_key_pem(LineEnding::default())
                .expect("Could not serialize public key"),
        )
    }
}

impl TryFrom<&PublicKey> for RsaPublicKey {
    type Error = String;

    fn try_from(key: &PublicKey) -> Result<Self, Self::Error> {
        RsaPublicKey::from_public_key_pem(&key.0).map_err(|e| format!("Not a valid pub key: {e}"))
    }
}

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

    /// Integer value to benchmark
    #[arg(short, long)]
    value: u64,
}

#[derive(NetworkBehaviour)]
struct MyBehaviour {
    upnp: upnp::tokio::Behaviour,
    gossipsub: gossipsub::Behaviour,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum Msg {
    Join(PublicKey, String),
    Participants(HashMap<PublicKey, String>),
    LobbyNowClosed,
    Share {
        from: PublicKey,
        to: PublicKey,
        share: Vec<u8>,
    },
    Sum(PublicKey, u64),
    Result(f64),
}

impl Msg {
    fn serialize(&self) -> Result<Vec<u8>, Box<dyn Error>> {
        Ok(bincode::serialize(&self)?)
    }
}

#[derive(Debug, Clone, Copy)]
enum Phase {
    WaitingForParticipants,
    ConfirmingParticipants,
    SendingShares,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init();
    let Args {
        address,
        name,
        value: secret_value,
    } = Args::parse();

    let mut swarm = libp2p::SwarmBuilder::with_new_identity()
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
        println!("Joining session at {addr}...");
    }

    println!("Generating public/private key pair...");
    let mut rng = rand::thread_rng();
    let private_key = RsaPrivateKey::new(&mut rng, KEY_BITS).expect("failed to generate a key");
    let signing_key = SigningKey::<Sha256>::new(private_key.clone());
    let pub_key = PublicKey::from(RsaPublicKey::from(&private_key));

    let mut phase = Phase::WaitingForParticipants;
    let mut stdin = io::BufReader::new(io::stdin()).lines();
    let mut participants = HashMap::<PublicKey, String>::new();
    let mut sent_shares = HashMap::<PublicKey, u64>::new();
    let mut received_shares = HashMap::<PublicKey, Vec<u8>>::new();
    let mut sums = HashMap::<PublicKey, u64>::new();
    let mut result = None;
    enum Event {
        Upnp(upnp::Event),
        StdIn(String),
        Msg(Msg),
    }
    let confirm_msg = "Please double-check the participants. Do you want to join the benchmark? [Y/n]";
    let starting_msg = "Starting benchmark with the current participants...";
    loop {
        if let Phase::SendingShares = phase {
            if swarm.behaviour().gossipsub.all_peers().count() == 0 {
                std::process::exit(0);
            }
            if sent_shares.is_empty() {
                for public_key in participants.keys() {
                    if *public_key == pub_key.clone() {
                        continue;
                    }
                    let share: u64 = rand::random();
                    let receiver_public_key = RsaPublicKey::try_from(public_key)?;

                    let mut encrypted_share = receiver_public_key
                        .encrypt(&mut rng, Pkcs1v15Encrypt, &share.to_be_bytes())
                        .map_err(|e| format!("failed to encrypt: {e}"))?;

                    let signature = signing_key
                        .sign_with_rng(&mut rng, &encrypted_share)
                        .to_vec();

                    encrypted_share.extend(signature);

                    sent_shares.insert(public_key.clone(), share.clone());
                    let msg = Msg::Share {
                        to: public_key.clone(),
                        from: pub_key.clone(),
                        share: encrypted_share,
                    }
                    .serialize()?;
                    swarm
                        .behaviour_mut()
                        .gossipsub
                        .publish(topic.clone(), msg)?;
                }
            }
            if received_shares.len() == participants.len() - 1 {
                let mut sent_sum: u64 = 0;
                for share in sent_shares.values() {
                    sent_sum = sent_sum.wrapping_add(*share);
                }
                let masked_secret: u64 = secret_value.wrapping_sub(sent_sum);
                let mut public_sum = masked_secret;

                for (sender_pub_key, encrypted_share) in &received_shares {
                    let share = &encrypted_share[..256];
                    let signature = &encrypted_share[256..];
                    let signature = Signature::try_from(signature)
                        .map_err(|e| format!("Not a valid signature: {e}"))?;

                    let pub_key_sender = RsaPublicKey::try_from(sender_pub_key)?;
                    let verifying_key = VerifyingKey::<Sha256>::new(pub_key_sender);

                    verifying_key
                        .verify(&share, &signature)
                        .map_err(|e| format!("Verification of msg sender failed: {e}"))?;

                    let decrypted_msg = private_key
                        .decrypt(Pkcs1v15Encrypt, &share)
                        .map_err(|e| format!("failed to decrypt: {e}"))?;
                    let received_bytes: &[u8] = &decrypted_msg;
                    let received_share = u64::from_be_bytes(received_bytes.try_into().unwrap());
                    public_sum = public_sum.wrapping_add(received_share);
                }

                let msg = Msg::Sum(pub_key.clone(), public_sum).serialize()?;
                if is_leader {
                    sums.insert(pub_key.clone(), public_sum);
                }
                swarm
                    .behaviour_mut()
                    .gossipsub
                    .publish(topic.clone(), msg)?;
            }
            if is_leader && sums.len() == participants.len() {
                let mut sum: u64 = 0;
                for s in sums.values() {
                    sum = sum.wrapping_add(*s);
                }
                let avg = sum as f64 / participants.len() as f64;
                let msg = Msg::Result(avg).serialize()?;
                swarm
                    .behaviour_mut()
                    .gossipsub
                    .publish(topic.clone(), msg)?;
                if result.is_none() {
                    result = Some(avg);
                    println!("");
                    println!("The average of the benchmarked values is: {avg:.2}");
                }
            }
        }
        let ev = select! {
            Ok(Some(line)) = stdin.next_line() => {
                Event::StdIn(line)
            }
            ev = swarm.select_next_some() => match ev {
                SwarmEvent::Behaviour(MyBehaviourEvent::Upnp(ev)) => Event::Upnp(ev),
                SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                    propagation_source,
                    message,
                    ..
                })) => {
                    let Ok(msg) = bincode::deserialize::<Msg>(&message.data) else {
                        error!("Received invalid message from {propagation_source}");
                        continue;
                    };
                    if let Msg::Share { from, to, share } = msg.clone() {
                        if to == pub_key.clone() {
                            if participants.contains_key(&from) {
                                received_shares.insert(from, share);
                            }
                        }
                    }
                    Event::Msg(msg)
                },
                ev => {
                    info!("{ev:?}");
                    continue;
                }
            },
        };
        match (phase, ev) {
            (Phase::WaitingForParticipants, Event::StdIn(_)) if is_leader => {
                if participants.len() < 3 {
                    println!("Cannot start yet, at least 3 participants are needed to ensure privacy.");
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
            (Phase::ConfirmingParticipants, Event::StdIn(line)) => {
                if line.trim().is_empty() || line.trim().to_lowercase() == "y" {
                    println!("{starting_msg}");
                    phase = Phase::SendingShares;
                } else if line.trim().to_lowercase() == "n" {
                    std::process::exit(0);
                } else {
                    println!("{confirm_msg}");
                }
            }
            (_, Event::StdIn(_)) => {}
            (Phase::WaitingForParticipants, Event::Upnp(upnp::Event::NewExternalAddr(addr))) => {
                if is_leader {
                    println!("A new session has been started, others can join using the following command:");
                    println!("cargo run -- --address={addr} --name=alias --value=");
                    println!("");
                    println!(
                        "Press ENTER to start the benchmark once all participants have joined."
                    );
                    println!("");
                    println!("-- Participants --");
                    println!("{pub_key} - {name}");
                } else {
                    let msg = Msg::Join(pub_key.clone(), name.clone()).serialize()?;
                    swarm
                        .behaviour_mut()
                        .gossipsub
                        .publish(topic.clone(), msg)?;
                    println!("");
                    println!("-- Participants --");
                    println!("{pub_key} - {name}");
                }
                swarm.behaviour_mut().gossipsub.subscribe(&topic)?;
                participants.insert(pub_key.clone(), name.clone());
            }
            (_, Event::Upnp(upnp::Event::GatewayNotFound)) => {
                error!("Gateway does not support UPnP");
                break;
            }
            (_, Event::Upnp(upnp::Event::NonRoutableGateway)) => {
                error!("Gateway is not exposed directly to the public Internet, i.e. it itself has a private IP address.");
                break;
            }
            (_, Event::Upnp(ev)) => info!("{ev:?}"),
            (Phase::WaitingForParticipants, Event::Msg(msg)) => match msg {
                Msg::Join(public_key, name) => {
                    if is_leader {
                        println!("{public_key} - {name}");
                        participants.insert(public_key, name);
                        let msg = Msg::Participants(participants.clone()).serialize()?;
                        if let Err(e) = swarm.behaviour_mut().gossipsub.publish(topic.clone(), msg)
                        {
                            error!("Could not publish to gossipsub: {e:?}");
                        }
                    }
                }
                Msg::Participants(all_participants) => {
                    for (public_key, name) in all_participants.iter() {
                        if !participants.contains_key(public_key) {
                            println!("{public_key} - {name}");
                        }
                    }
                    participants = all_participants;
                }
                Msg::LobbyNowClosed => {
                    if is_leader {
                        error!("This message should never be sent to the benchmark leader!");
                    } else if participants.len() < 3 {
                        eprintln!("Someone tried to start a benchmark with < 3 participants!");
                        std::process::exit(1);
                    } else {
                        phase = Phase::ConfirmingParticipants;
                        println!("");
                        println!("{confirm_msg}");
                    }
                }
                Msg::Share { .. } => {}
                Msg::Sum(_, _) => {
                    error!("Received sum from participant while still waiting for participants to join!");
                    std::process::exit(1);
                }
                Msg::Result(_) => {
                    error!("Received result while still waiting for participants to join!");
                    std::process::exit(1);
                }
            },
            (Phase::SendingShares, Event::Msg(msg)) => match msg {
                Msg::Join(_, _) | Msg::Participants(_) | Msg::LobbyNowClosed => {
                    println!(
                        "Already waiting for shares, but some participant still tried to join!"
                    );
                    continue;
                }
                Msg::Share { .. } => {}
                Msg::Sum(public_key, sum) => {
                    if is_leader {
                        sums.insert(public_key, sum);
                    }
                }
                Msg::Result(avg) => {
                    println!("");
                    println!("The average of the benchmarked values is: {avg:.2}");
                    std::process::exit(0);
                }
            },
            (Phase::ConfirmingParticipants, _) => {}
        }
    }
    Ok(())
}
