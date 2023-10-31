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

To start a benchmarking session, you will need at least three participants to ensure privacy.

```sh
sine-benchmark --name=<your-name> --input=<file.json>
```

You are now the `benchmark leader` and will get the following prompt:

```sh
Generating public/private key pair...
A new session has been started, others can join using the following command:
sine-benchmark --address=ip4/<xx>.<xxx>.<xx>.<xxx>/tcp/<xxxxx> --name=<your_alias> --input=<file.json>
```

Communicate your address to other participants by sharing the command above. They will then be able to join the session:

```sh
sine-benchmark --address=/ip4/<xx>.<xxx>.<xx>.<xxx>/tcp/<xxxxx> --name=<your-name> --input=<file.json>
```

**Note:** The input files of all participants should have the exact same keys.

Everyone will be able to see the list of participants, indicating their hashed key and their name, e.g.:
```sh
e162c856 416cb0a9 c78155eb b8d3c9dd - foo
271270ba 7d268105 9ae5e96e 23b8f7e4 - bar
25431517 e42910b2 e359702e 20c61f49 - baz
```

Once all participants have joined, the `benchmark leader` can hit `Enter` to begin the benchmark. At that point, other participants will receive the following prompt:

```sh
Please double-check the participants. Do you want to join the benchmark? [Y/n]
```

Checking the participants' hashed keys is a security measure to ensure that no man-in-the-middle attack is taking place.

Once all participants have entered `Y`, the benchmark will start. Soon the average results will be displayed.

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
