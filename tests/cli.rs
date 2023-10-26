use std::{
    io::{BufRead, BufReader, Error, ErrorKind},
    process::{Child, Command, Stdio},
};

use assert_cmd::prelude::{CommandCargoExt, OutputAssertExt}; // Run programs

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

// #[test]
// fn invalid_address() -> Result<(), Box<dyn std::error::Error>> {
//     new_command("foo", Some("bar"), "tests/test_files/valid_json.json")?
//         .assert()
//         .failure()
//         .stderr(predicates::str::contains("InvalidMultiaddr"));
//     Ok(())
// }

// #[test]
// fn session() -> Result<(), Box<dyn std::error::Error>> {
//     let mut new_session = new_command("foo", None, "tests/test_files/valid_json.json")?;

//     let Some(stdout) = new_session.spawn()?.stdout else {
//         return Err(Box::new(Error::new(ErrorKind::Other, "No stdout found")));
//     };

//     let mut address: String = "".to_string();

//     let reader = BufReader::new(stdout);

//     reader.lines().for_each(|line| match line {
//         Ok(l) => {
//             if l.contains("--address=/ip4/") {
//                 address = l
//                     .split(" ")
//                     .find(|s| s.contains("--address=/ip4/"))
//                     .unwrap()
//                     .replace("--address=", "");
//             }
//             println!("{}", l);
//         }
//         Err(_) => {}
//     });

//     new_command("bar", Some(&address), "tests/test_files/valid_json.json")?.assert().success();
//     new_command("baz", Some(&address), "tests/test_files/valid_json.json")?.assert().success();

//     Ok(())
// }

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
