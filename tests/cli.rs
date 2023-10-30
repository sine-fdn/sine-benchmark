use std::{
    io::{BufRead, BufReader, BufWriter, Error, ErrorKind, Write},
    process::{Command, Stdio},
    thread::{self, sleep},
    time::Duration,
};

use assert_cmd::prelude::{CommandCargoExt, OutputAssertExt};

const CRATE_NAME: &str = "sine-benchmark";

#[test]
fn file_doesnt_exist() -> Result<(), Box<dyn std::error::Error>> {
    new_command("foo", None, "nonexisting_file.json")?
        .assert()
        .failure()
        .stderr(predicates::str::contains("No such file"));

    Ok(())
}

#[test]
fn wrong_file_format() -> Result<(), Box<dyn std::error::Error>> {
    new_command("foo", None, "tests/test_files/wrong_file_format.txt")?
        .assert()
        .failure()
        .stderr(predicates::str::contains("is not a valid JSON file"));
    Ok(())
}

#[test]
fn invalid_json() -> Result<(), Box<dyn std::error::Error>> {
    new_command("foo", None, "tests/test_files/invalid_json.json")?
        .assert()
        .failure()
        .stderr(predicates::str::contains("is not a valid JSON file"));
    Ok(())
}

#[test]
fn wrong_json_types() -> Result<(), Box<dyn std::error::Error>> {
    new_command("foo", None, "tests/test_files/wrong_types.json")?
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "with a map of string keys and integer number values",
        ));
    Ok(())
}

#[test]
fn no_session_at_address() -> Result<(), Box<dyn std::error::Error>> {
    new_command(
        "foo",
        Some("/ip4/0.0.0.0/tcp/12345"),
        "tests/test_files/valid_json.json",
    )?
    .assert()
    .failure()
    .stderr(predicates::str::contains("InsufficientPeers"));
    Ok(())
}

#[test]
fn invalid_address() -> Result<(), Box<dyn std::error::Error>> {
    new_command("foo", Some("bar"), "tests/test_files/valid_json.json")?
        .assert()
        .failure()
        .stderr(predicates::str::contains("InvalidMultiaddr"));
    Ok(())
}

#[test]
fn session() -> Result<(), Box<dyn std::error::Error>> {
    let mut new_session = new_command("foo", None, "tests/test_files/valid_json.json")?;

    let mut leader = new_session
        .stdout(Stdio::piped())
        .stdin(Stdio::piped())
        .spawn()?;
    let stdout = leader.stdout.take().unwrap();
    let reader = BufReader::new(stdout);
    let stdin = leader.stdin.take().unwrap();
    let mut writer = BufWriter::new(stdin);
    let mut lines = reader.lines();

    let address = loop {
        if let Some(Ok(l)) = lines.next() {
            println!("foo > {}", l);
            if l.contains("--address=/ip4/") {
                break l
                    .split(" ")
                    .find(|s| s.contains("--address=/ip4/"))
                    .unwrap()
                    .replace("--address=", "");
            }
        }
    };

    let mut threads = vec![];
    for name in ["bar", "baz"] {
        let address = address.clone();
        threads.push(thread::spawn(move || {
            let mut participant =
                new_command(name, Some(&address), "tests/test_files/valid_json.json")
                    .unwrap()
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .spawn()
                    .unwrap();

            let stdout = participant.stdout.take().unwrap();
            let reader = BufReader::new(stdout);
            let stdin = participant.stdin.take().unwrap();
            let mut writer = BufWriter::new(stdin);
            let mut lines = reader.lines();

            while let Some(Ok(l)) = lines.next() {
                println!("{name} > {l}");

                if l.contains("Do you want to join the benchmark?") {
                    sleep(Duration::from_millis(200));
                    writeln!(writer, "y").unwrap();
                    writer.flush().unwrap();
                }

                if l.contains("results") {
                    participant.kill().unwrap();
                    return;
                }
            }
        }));
    }

    let mut participant_count = 1;
    let mut example1_correct = false;
    let mut example2_correct = false;
    let mut example3_correct = false;
    while let Some(Ok(l)) = lines.next() {
        println!("foo > {}", l);
        if l.contains("- bar") || l.contains("- baz") {
            participant_count += 1;
        }
        if participant_count == 3 {
            sleep(Duration::from_millis(200));
            writeln!(writer, "").unwrap();
            writer.flush().unwrap();
        }
        if l.contains("example1: ") {
            example1_correct = l.split(" ").last().unwrap() == "10.00";
        }
        if l.contains("example2: ") {
            example2_correct = l.split(" ").last().unwrap() == "15.00";
        }
        if l.contains("example3: ") {
            example3_correct = l.split(" ").last().unwrap() == "18.00";
        }
        if example1_correct && example2_correct && example3_correct {
            break;
        }
    }

    sleep(Duration::from_millis(200));
    leader.kill()?;

    for t in threads {
        t.join().unwrap();
    }

    if example1_correct && example2_correct && example3_correct {
        Ok(())
    } else {
        Err(Box::new(Error::new(ErrorKind::Other, "Wrong results")))
    }
}

fn new_command(
    name: &str,
    address: Option<&str>,
    input: &str,
) -> Result<Command, Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin(CRATE_NAME)?;

    cmd.args(["--name", name]);

    match address {
        Some(addr) => {
            cmd.args(["--address", addr]);
        }
        None => {}
    }

    cmd.args(["--input", input]);

    Ok(cmd)
}
