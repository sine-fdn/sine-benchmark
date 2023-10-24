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
use std::{collections::HashMap, error::Error, path::PathBuf, time::Duration};
use tokio::{
    fs,
    io::{self, AsyncBufReadExt},
    select,
};

const KEY_BITS: usize = 2048;
const MAX_MSG_SIZE_BYTES: usize = 245;

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

    /// JSON file with key-value pairs to benchmark
    #[arg(short, long)]
    input: PathBuf,
}

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
    Sum(PublicKey, HashMap<String, i64>),
    Result(HashMap<String, i64>),
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
        input,
    } = Args::parse();
    let Ok(_) = fs::metadata(&input).await else {
        eprintln!("No such file: {}", input.display());
        eprintln!("The input must be a JSON file with key-value pairs.");
        std::process::exit(1);
    };
    let input = match fs::read_to_string(&input).await {
        Err(e) => {
            eprintln!("Could not read file {}: {}", input.display(), e);
            std::process::exit(1);
        }
        Ok(file) => match serde_json::from_str::<HashMap<String, i64>>(&file) {
            Ok(json) => json,
            Err(_) => {
                eprintln!("The file {} is not a valid JSON file with a map of string keys and integer number values.", input.display());
                std::process::exit(1);
            }
        },
    };

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
    let mut sent_shares = HashMap::<PublicKey, HashMap<&String, i64>>::new();
    let mut received_shares = HashMap::<PublicKey, Vec<u8>>::new();
    let mut sums = HashMap::<PublicKey, HashMap<String, i64>>::new();
    let mut result = None;
    enum Event {
        Upnp(upnp::Event),
        StdIn(String),
        Msg(Msg),
    }
    let confirm_msg =
        "Please double-check the participants. Do you want to join the benchmark? [Y/n]";
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
                    let mut msg = vec![];
                    let mut shares = HashMap::new();
                    for key in input.keys() {
                        let share: i64 = rand::random();
                        shares.insert(key, share);

                        let mut chunk = [0u8; MAX_MSG_SIZE_BYTES];
                        let key_len = key.as_bytes().len() as i64;
                        let max_size = MAX_MSG_SIZE_BYTES - 16;
                        if (key_len as usize) > MAX_MSG_SIZE_BYTES - 16 {
                            eprintln!("Key '{key}' ({key_len} bytes) exceeds maximum key size of {max_size} bytes");
                            std::process::exit(1);
                        }
                        chunk[..8].copy_from_slice(&key_len.to_be_bytes());
                        chunk[8..16].copy_from_slice(&share.to_be_bytes());
                        chunk[16..16 + (key_len as usize)].copy_from_slice(key.as_bytes());

                        assert_eq!(chunk.len(), MAX_MSG_SIZE_BYTES);

                        let receiver_public_key = RsaPublicKey::try_from(public_key)?;

                        let chunk = receiver_public_key
                            .encrypt(&mut rng, Pkcs1v15Encrypt, &chunk)
                            .map_err(|e| format!("failed to encrypt: {e}"))?;
                        assert_eq!(chunk.len(), KEY_BITS / 8);

                        let signature = signing_key.sign_with_rng(&mut rng, &chunk).to_vec();
                        assert_eq!(signature.len(), KEY_BITS / 8);

                        msg.extend(chunk);
                        msg.extend(signature);
                    }
                    sent_shares.insert(public_key.clone(), shares);
                    let msg = Msg::Share {
                        to: public_key.clone(),
                        from: pub_key.clone(),
                        share: msg,
                    }
                    .serialize()?;
                    swarm
                        .behaviour_mut()
                        .gossipsub
                        .publish(topic.clone(), msg)?;
                }
            }
            if received_shares.len() == participants.len() - 1 {
                let mut sent_sums: HashMap<&String, i64> = HashMap::new();
                for share in sent_shares.values() {
                    for (key, share) in share.iter() {
                        let sent_sum: i64 = sent_sums.get(*key).copied().unwrap_or_default();
                        *sent_sums.entry(key).or_default() = sent_sum.wrapping_add(*share);
                    }
                }
                let mut public_sums = HashMap::new();
                for (key, sent_sum) in sent_sums {
                    let secret_value = input.get(key).unwrap();
                    let masked_secret: i64 = secret_value.wrapping_sub(sent_sum);
                    public_sums.insert(key.clone(), masked_secret);
                }
                for (sender_pub_key, enc_msg) in &received_shares {
                    let pub_key_sender = RsaPublicKey::try_from(sender_pub_key)?;
                    let verifying_key = VerifyingKey::<Sha256>::new(pub_key_sender);

                    assert_eq!(enc_msg.len() % ((KEY_BITS / 8) * 2), 0);
                    for i in (0..enc_msg.len()).step_by((KEY_BITS / 8) * 2) {
                        if enc_msg.len() < i + (KEY_BITS / 8) * 2 {
                            eprintln!("Unexpected end of message at offset");
                            std::process::exit(1);
                        }
                        let chunk = &enc_msg[i..i + KEY_BITS / 8];
                        let signature = &enc_msg[i + KEY_BITS / 8..i + (KEY_BITS / 8) * 2];
                        let signature = Signature::try_from(signature)
                            .map_err(|e| format!("Not a valid signature: {e}"))?;

                        verifying_key
                            .verify(&chunk, &signature)
                            .map_err(|e| format!("Verification of msg sender failed: {e}"))?;

                        let chunk = private_key
                            .decrypt(Pkcs1v15Encrypt, &chunk)
                            .map_err(|e| format!("failed to decrypt: {e}"))?;

                        let key_len = i64::from_be_bytes(chunk[..8].try_into().unwrap()) as usize;
                        if key_len > chunk.len() - 16 {
                            eprintln!("Invalid length of key: {key_len} bytes");
                            std::process::exit(1);
                        }
                        let share = i64::from_be_bytes(chunk[8..16].try_into().unwrap());
                        let key = String::from_utf8(chunk[16..16 + key_len].to_vec())
                            .map_err(|e| format!("Not a valid UTF-8 string: {e}"))?;
                        if let Some(public_sum) = public_sums.get_mut(&key) {
                            *public_sum = public_sum.wrapping_add(share);
                        } else {
                            eprintln!("Received invalid key {key} from one of the participants!");
                            std::process::exit(1);
                        }
                    }
                }

                let msg = Msg::Sum(pub_key.clone(), public_sums.clone()).serialize()?;
                if is_leader {
                    sums.insert(pub_key.clone(), public_sums);
                }
                swarm
                    .behaviour_mut()
                    .gossipsub
                    .publish(topic.clone(), msg)?;
            }
            if is_leader && sums.len() == participants.len() {
                let mut results = HashMap::new();
                for s in sums.values() {
                    for (key, s) in s {
                        let result: i64 = results.get(key).copied().unwrap_or_default();
                        *results.entry(key.clone()).or_default() = result.wrapping_add(*s);
                    }
                }
                let msg = Msg::Result(results.clone()).serialize()?;
                swarm
                    .behaviour_mut()
                    .gossipsub
                    .publish(topic.clone(), msg)?;
                if result.is_none() {
                    println!("");
                    println!("Average results:");
                    for (key, result) in results.iter() {
                        let avg = *result as f64 / participants.len() as f64;
                        println!("{key}: {avg:.2}")
                    }
                    result = Some(results);
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
                    println!(
                        "Cannot start yet, at least 3 participants are needed to ensure privacy."
                    );
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
                    println!("cargo run -- --address={addr} --name=<your_alias> --input=<file.json>");
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
                Msg::Result(results) => {
                    println!("");
                    println!("Average results:");
                    for (key, result) in results {
                        let avg = result as f64 / participants.len() as f64;
                        println!("{key}: {avg:.2}")
                    }
                    std::process::exit(0);
                }
            },
            (Phase::ConfirmingParticipants, _) => {}
        }
    }
    Ok(())
}
