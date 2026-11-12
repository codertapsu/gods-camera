//! What Windows says about the network: its firewall's rules, this
//! computer's name, and an announcement of the hub.
#![cfg(windows)]

use std::path::Path;
use std::time::Duration;

use gc_win::{addresses, computer_name, firewall, Announced, Announcement};

#[test]
fn this_computer_has_a_name() {
    let name = computer_name();
    assert!(!name.is_empty());
    assert!(name.chars().all(|c| !c.is_control()), "a name is one line of text");
}

/// Whatever addresses this computer has on its networks, none is its own
/// loopback or a placeholder. A computer without a network has none.
#[test]
fn this_computers_addresses_are_ones_another_device_could_call() {
    for address in addresses() {
        assert!(!address.is_loopback() && !address.is_unspecified() && !address.is_broadcast(), "{address}");
    }
}

/// Reading the firewall's rules needs no administrator, and a program that
/// was never installed has no rule.
#[test]
fn the_firewall_has_no_rule_for_a_program_nobody_installed() {
    let answer = firewall(Path::new(r"C:\Nowhere\gods-camera-test\NoSuchProgram.exe"), 47801).expect("Windows Firewall answers");
    assert!(!answer.private_allowed && !answer.public_settled);
    assert!(!answer.ready());
}

/// Announces a made-up hub on the network this computer is on, for a few
/// seconds. Run by hand:
///
///     cargo test -p gc-win --test network -- --ignored --nocapture
///
/// While it waits, a phone or another computer on the network can look for
/// `_godscamera._tcp` and should see it.
#[test]
#[ignore = "announces a service on the real network for a few seconds"]
fn a_hub_can_be_announced_and_taken_back() {
    let name = format!("gc-test-{}", std::process::id());
    let announcement = Announcement::start(&name, 47999, &"ab".repeat(32)).expect("Windows takes the announcement");
    let state = announcement.settled(Duration::from_secs(10));
    println!("announced as {name}: {state:?}");
    assert_eq!(state, Announced::Up);
    std::thread::sleep(Duration::from_secs(3));
    drop(announcement);
}
