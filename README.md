# SINE Benchmark

This repository contains a **privacy preserving** benchmarking tool that allows for **peer-to-peer** benchmarking against group average without disclosing inputs.

<img alt="SINE Logo" height="150" align="right" src="https://user-images.githubusercontent.com/358580/204315360-9e4916df-5080-4e7c-bd5b-7e002309b9db.png">

## Usage

### Installation

Install the benchmarking tool using `cargo install` from git over either `ssh` or `https`:

```sh
cargo install --git ssh://git@github.com/sine-fdn/sine-benchmark.git
```

```sh
cargo install --git https://github.com/sine-fdn/sine-benchmark.git
```

### Running a Benchmark

_**Note:** You will need at least three participants to run a benchmark._

Each participant is identified by (a freely chosen) name and needs to specify their private inputs in a JSON file as pairs of string keys and number values (with a maximum precision of 2 decimal digits), for example:

```json
{
    "revenue": 1234.56,
    "costs": 1000,
}
```

The first participant can then start the benchmark:

```sh
$ sine-benchmark --name=alice --input=inputs.json
Generating public/private key pair...
Your public key is: 97bd80c5 ff6e8a34 e1813f97 61a47898
A new session has been started, others can join using the following command:
sine-benchmark --address=/ip4/161.230.165.79/tcp/61958 --name=<your_alias> --input=<file.json>

Press ENTER to start the benchmark once all participants have joined.

-- Participants --
97bd80c5 ff6e8a34 e1813f97 61a47898 - alice
```

By sharing the address, other participants can then join the benchmark:

```sh
$ sine-benchmark --address=/ip4/161.230.165.79/tcp/61958 --name=bob --input=inputs.json
Joining session at /ip4/161.230.165.79/tcp/61958...
Generating public/private key pair...
Your public key is: d87e1657 5a59b72e 0df57a0f 95fbb993

-- Participants --
d87e1657 5a59b72e 0df57a0f 95fbb993 - bob
97bd80c5 ff6e8a34 e1813f97 61a47898 - alice
```

_**Note:** The input files of all participants need to have the same string keys._

Once all participants have joined, the `benchmark leader` can hit `Enter` to begin the actual benchmarking process:

```sh
Press ENTER to start the benchmark once all participants have joined.

-- Participants --
97bd80c5 ff6e8a34 e1813f97 61a47898 - alice
d87e1657 5a59b72e 0df57a0f 95fbb993 - bob
34400918 89b51364 704626b4 faec8e42 - carol

Starting benchmark with the current participants...
```

The other participants are then asked to confirm the list of participants. At this point, no data is exchanged yet. Everyone is able to see the list of participants, showing their hashed public key and their chosen name. It is good practice to manually double-check the participants' hashed keys to ensure that no man-in-the-middle attack is taking place:

```sh
-- Participants --
d87e1657 5a59b72e 0df57a0f 95fbb993 - bob
97bd80c5 ff6e8a34 e1813f97 61a47898 - alice
34400918 89b51364 704626b4 faec8e42 - carol

Please double-check the participants. Do you want to join the benchmark? [Y/n]
y
Ok, joining benchmarking with the current participants...
```

Once all participants have confirmed, the benchmark is started and the average of all the inputs is calulated and displayed:

```sh
Average results:
revenue: 1234.56
costs: 1000
```

## Technical Description

SINE Benchmark uses **Secret Sharing** and **Public Key Encryption** to keep the input values private, as well as a **peer-to-peer** connection to avoid the need to deploy and maintain a server.

### Secret Sharing and Public Key Encryption

SINE Benchmark is a privacy preserving benchmarking CLI tool, using secret sharing.

The protocol used in SINE Benchmark can be illustrated by this simple example:

<img width="1670" alt="Multi-Party Computation Example" src="https://github.com/sine-fdn/sine-benchmark/assets/100690574/10619fc4-7f21-4127-8d29-2f5714203020">


As in the example, the CLI tool takes a set of values (defined in a json to be provided by the user) and for each of the values generates a number of random shares identical to the number of other participants.

When they join a benchmarking session, each participant is given a public and a private key. These will be used to **encrypt** and **sign** the shares before sending them over:
- each share is encrypted with the public key of the participant to whom it is directed;
- and then signed with the private key of the sender.

The encrypted messages are sent to all other users but only those to whom they are directed can decrypt them. The receivers then proceed to verify the signatures and decrypt the messages.

Once in possession of all shares, each participant can add them to their secret share (i.e., the result of subtracting the shares to their private value) yielding their sum.

Sums cannot be traced back to the private values of participants and are, therefore, sent as plain text.

With the sums of all participants in their possession, each participant can calculate the average locally.

### Peer-to-peer

SINE Benchmark uses peer-to-peer technology to allow for benchmarking without a server.

It uses [libp2p](https://github.com/libp2p/rust-libp2p) and, in particular,
- the [upnp network behaviour](https://github.com/libp2p/rust-libp2p/tree/master/examples/upnp); and
- the [gossipsub](https://github.com/libp2p/specs/tree/master/pubsub/gossipsub) Publish/Subscribe (pubsub) protocol.

Benchmarking sessions correspond to pubsub _topics_, in which messages are sent (but cannot be decrypted by) all members at the same time.

Thus, participants only need to establish connection with one peer. That peer will cue the start of the benchmark and is considered the `benchmark leader`.

**Note:** The `benchmark leader` is a "normal" participant with access to exactly the same information as all others.
