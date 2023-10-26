# SINE Benchmark

This repository contains a **privacy preserving** benchmarking tool that allows for **peer-to-peer** benchmarking against group average without disclosing inputs.

<img alt="SINE Logo" height="150" align="right" src="https://user-images.githubusercontent.com/358580/204315360-9e4916df-5080-4e7c-bd5b-7e002309b9db.png">

## Technical Description

SINE Benchmark uses **Multi-Party Computation** and **Public Key Encryption** to keep the input values private, as well as a **peer-to-peer** connection to avoid the need to deploy and maintain a server.

### Secure Multi-Party Computation and Public Key Encryption

SINE Benchmark is a privacy preserving benchmarking CLI tool. This is achieved by the use of [Secure Multi-Party Computation](https://sine.foundation/library/002-smpc).

The protocol used in SINE Benchmark can be illustrated by this simple example:

<img width="1670" alt="MPC" src="https://github.com/sine-fdn/sine-benchmark/assets/100690574/10619fc4-7f21-4127-8d29-2f5714203020">

The CLI tool takes a set of values (defined in a json to be provided by the user) and for each of the values generates a number of random shares identical to the number of other participants.

When they join a benchmarking session, each participant is given a public and a private key. These will be used to **encrypt** and **sign** the shares before sending them over:
- each share is encrypted with the public key of the participant to whom it is directed;
- and then signed with the private key of the sender.

The encrypted messages are then sent to all other users but only those to whom they are directed can decrypt them. The receivers then proceed to verify the signatures and decrypt the messages.

Once in possession of all shares, each participant can add them to their secret share (i.e., the result of subtracting the shares to their private value) yielding their sum.

The sums can be sent as plain text, as they cannot be traced back to the private values of participants.

Putting the sums together and dividing them by the number of participants results in the average of each value, achieved without sharing any sensitive data.

### Peer-to-peer

SINE Benchmark uses peer-to-peer technology to allow for benchmarking without the need for a centralized server.

It uses [libp2p](https://github.com/libp2p/rust-libp2p) and, in particular,
- the [upnp network behaviour](https://github.com/libp2p/rust-libp2p/tree/master/examples/upnp); and
- the [gossipsub](https://github.com/libp2p/specs/tree/master/pubsub/gossipsub) Publish/Subscribe (pubsub) protocol.

Benchmarking sessions correspond to pubsub _topics_, in which messages are sent (but cannot be decrypted by) all members at the same time.

Thus, participants only need to establish connection with one peer, who will determine the beginning of the benchmark. That peer will cue the start of the benchmark and for that reason is considered the `benchmark leader`. It is, however, important to note that the `benchmark leader` is one of the participants and has the exact same information as they do.

## Usage

To start a benchmarking session, you will need:
- At least three participants (these can be different machines or different terminals on the same machine);
- A valid json file with an object with string keys and integer or float number values;

To start a benchmarking session, run:

```sh
cargo run -- --name=<your-name> --input=<file.json>
```

You are now the `benchmark leader` and will get the following prompt:

```sh
Generating public/private key pair...
A new session has been started, others can join using the following command:
cargo run -- --address=ip4/<xx>.<xxx>.<xx>.<xxx>/tcp/<xxxxx> --name=<your_alias> --input=<file.json>
```

Communicate your address to other participants by copying the command above (or only the part concerning the address) an sending it to them. They will then be able to join the session.

To join an ongoing session, run:

```sh
cargo run -- --address=/ip4/<xx>.<xxx>.<xx>.<xxx>/tcp/<xxxxx> --name=<your-name> --input=<file.json>
```

**Note:** The input files of all participants should have the exact same keys.

As participants join, everyone will be able to see the list of participants, indicating their hashed key and their name, e.g.:
```sh
e162c856 416cb0a9 c78155eb b8d3c9dd - foo
271270ba 7d268105 9ae5e96e 23b8f7e4 - bar
25431517 e42910b2 e359702e 20c61f49 - baz
```

Once all participants have joined, the `benchmark leader` can hit `Enter` to begin the benchmark. At that point, other participants will receive the following prompt:

```sh
Please double-check the participants. Do you want to join the benchmark? [Y/n]
```

Checking the participant hashed keys is very important to ensure that no man-in-the-middle attack is taking place.

Once all participants have entered `Y`, the benchmark will start. Soon the average results will be displayed.
