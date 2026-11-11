//! The pairing exchange, both sides in one process.

use gc_pair::{certificate_fingerprint, Grant, Initiator, PairError, PairingCode, Responder};

fn hub_certificate() -> String {
    certificate_fingerprint(b"the hub's certificate")
}

fn grant() -> Grant {
    Grant { device_id: "0d9f6e7a-52b1-4c1e-9a37-1f1f0c6f2f10".into(), token: "5e".repeat(32) }
}

fn code(digits: &str) -> PairingCode {
    PairingCode::parse(digits).unwrap()
}

/// Every message of one exchange, as it would cross the network.
struct Wire {
    device_name: String,
    device_hello: Vec<u8>,
    hub_hello: Vec<u8>,
    device_confirmation: Vec<u8>,
    hub_confirmation: Vec<u8>,
    sealed_grant: Vec<u8>,
}

/// Run a whole exchange. The device types `typed` and sees `seen` as the
/// certificate; the hub shows `shown` and owns `owned`.
fn exchange(typed: &str, shown: &str, seen: &str, owned: &str) -> Result<(Grant, Wire), PairError> {
    let device = Initiator::new(&code(typed), "Galaxy Tab S9+")?;
    let device_name = device.device_name().to_string();
    let device_hello = device.hello().to_vec();
    let (hub, hub_hello) = Responder::start(&code(shown), &device_name, &device_hello, owned)?;
    let (device, device_confirmation) = device.finish(&hub_hello, seen)?;
    let (hub_confirmation, sealed_grant) = hub.verify(&device_confirmation)?.grant(&grant());
    let granted = device.accept(&hub_confirmation, &sealed_grant)?;
    Ok((granted, Wire { device_name, device_hello, hub_hello, device_confirmation, hub_confirmation, sealed_grant }))
}

#[test]
fn the_right_code_pairs_and_delivers_the_grant() {
    let (granted, wire) = exchange("482913", "482913", &hub_certificate(), &hub_certificate()).unwrap();
    assert_eq!(granted, grant());
    assert_eq!(wire.device_name, "Galaxy Tab S9+");
    assert_eq!(wire.device_confirmation.len(), 32);
    assert_eq!(wire.hub_confirmation.len(), 32);
    // The two confirmations differ, so neither side can answer with what it was sent.
    assert_ne!(wire.device_confirmation, wire.hub_confirmation);

    // Nothing on the wire holds the code, the token, or anything that repeats.
    let (_, again) = exchange("482913", "482913", &hub_certificate(), &hub_certificate()).unwrap();
    assert_ne!(wire.device_hello, again.device_hello);
    assert_ne!(wire.hub_hello, again.hub_hello);
    assert_ne!(wire.device_confirmation, again.device_confirmation);
    assert_ne!(wire.sealed_grant, again.sealed_grant);
    let token = grant().token;
    let everything = [&wire.device_hello[..], &wire.hub_hello, &wire.device_confirmation, &wire.hub_confirmation, &wire.sealed_grant].concat();
    assert!(!everything.windows(token.len()).any(|window| window == token.as_bytes()));
}

#[test]
fn a_name_is_tidied_by_the_device_and_must_arrive_unchanged() {
    let device = Initiator::new(&code("111111"), "  Máy tính\tbảng  ").unwrap();
    assert_eq!(device.device_name(), "Máy tính bảng");
    assert!(matches!(Initiator::new(&code("111111"), " \n "), Err(PairError::InvalidName)));
    // The hub signs what it received; it does not correct it.
    let untidy = Responder::start(&code("111111"), " padded ", device.hello(), &hub_certificate());
    assert!(matches!(untidy, Err(PairError::InvalidName)));
    let bad_fingerprint = Responder::start(&code("111111"), "tablet", device.hello(), "abcd");
    assert!(matches!(bad_fingerprint, Err(PairError::InvalidFingerprint)));
    let (hub, hub_hello) = Responder::start(&code("111111"), "tablet", device.hello(), &hub_certificate()).unwrap();
    assert_eq!(hub.device_name(), "tablet");
    assert!(matches!(device.finish(&hub_hello, "not a fingerprint"), Err(PairError::InvalidFingerprint)));
}

#[test]
fn a_wrong_code_fails_at_the_hub_before_it_reveals_anything() {
    let device = Initiator::new(&code("000001"), "tablet").unwrap();
    let (hub, hub_hello) = Responder::start(&code("000002"), "tablet", device.hello(), &hub_certificate()).unwrap();
    let (_, device_confirmation) = device.finish(&hub_hello, &hub_certificate()).unwrap();
    // The hub's only answer is a refusal: no confirmation, no grant.
    assert!(matches!(hub.verify(&device_confirmation), Err(PairError::Failed)));
}

#[test]
fn a_hub_that_does_not_know_the_code_is_not_trusted() {
    // A stand-in hub guesses the code and answers as the real one would.
    let device = Initiator::new(&code("314159"), "tablet").unwrap();
    let (_, hub_hello) = Responder::start(&code("271828"), "tablet", device.hello(), &hub_certificate()).unwrap();
    let (device, _) = device.finish(&hub_hello, &hub_certificate()).unwrap();
    // Whatever it sends back, the device refuses it.
    let (_, other) = exchange("271828", "271828", &hub_certificate(), &hub_certificate()).unwrap();
    assert!(matches!(device.accept(&other.hub_confirmation, &other.sealed_grant), Err(PairError::Failed)));
}

#[test]
fn someone_in_the_middle_with_another_certificate_fails() {
    // The device's connection ended at someone else, who passed the messages
    // on: the code is right, but the certificates differ.
    let theirs = certificate_fingerprint(b"somebody else's certificate");
    assert!(matches!(exchange("482913", "482913", &theirs, &hub_certificate()), Err(PairError::Failed)));

    // Seen from the device: a hub that signed another certificate is refused too.
    let device = Initiator::new(&code("482913"), "tablet").unwrap();
    let (hub, hub_hello) = Responder::start(&code("482913"), "tablet", device.hello(), &theirs).unwrap();
    let (device, _) = device.finish(&hub_hello, &hub_certificate()).unwrap();
    let (_, honest) = exchange("482913", "482913", &theirs, &theirs).unwrap();
    assert!(matches!(device.accept(&honest.hub_confirmation, &honest.sealed_grant), Err(PairError::Failed)));
    drop(hub);
}

#[test]
fn a_name_changed_on_the_way_fails() {
    let device = Initiator::new(&code("482913"), "tablet").unwrap();
    let (hub, hub_hello) = Responder::start(&code("482913"), "laptop", device.hello(), &hub_certificate()).unwrap();
    let (_, device_confirmation) = device.finish(&hub_hello, &hub_certificate()).unwrap();
    assert!(matches!(hub.verify(&device_confirmation), Err(PairError::Failed)));
}

/// Flip one bit of `bytes` at `index`.
fn flipped(bytes: &[u8], index: usize) -> Vec<u8> {
    let mut out = bytes.to_vec();
    out[index] ^= 0x01;
    out
}

/// One exchange with the right code, in which message number `which` passes
/// through `change` on its way: 0 the device's first message, 1 the hub's,
/// 2 the device's confirmation, 3 the hub's confirmation, 4 the sealed grant.
fn exchange_changing(which: usize, change: &dyn Fn(&[u8]) -> Vec<u8>) -> Result<Grant, PairError> {
    let fingerprint = hub_certificate();
    let pass = |index: usize, message: &[u8]| if index == which { change(message) } else { message.to_vec() };
    let device = Initiator::new(&code("482913"), "tablet")?;
    let device_hello = pass(0, device.hello());
    let (hub, hub_hello) = Responder::start(&code("482913"), "tablet", &device_hello, &fingerprint)?;
    let (device, device_confirmation) = device.finish(&pass(1, &hub_hello), &fingerprint)?;
    let (hub_confirmation, sealed_grant) = hub.verify(&pass(2, &device_confirmation))?.grant(&grant());
    device.accept(&pass(3, &hub_confirmation), &pass(4, &sealed_grant))
}

#[test]
fn any_changed_message_fails() {
    let names = ["device hello", "hub hello", "device confirmation", "hub confirmation", "grant"];
    for (which, name) in names.iter().enumerate() {
        // Unchanged, the exchange succeeds; this also measures the message.
        let length = std::cell::Cell::new(0);
        let unchanged = exchange_changing(which, &|message| {
            length.set(message.len());
            message.to_vec()
        });
        assert_eq!(unchanged, Ok(grant()), "{name}");
        let length = length.get();
        assert!(length >= 32, "{name} is {length} bytes");

        // One bit flipped, anywhere.
        for index in 0..length {
            assert_eq!(exchange_changing(which, &|m| flipped(m, index)), Err(PairError::Failed), "{name}, byte {index}");
        }
        // Cut short, emptied, or with a byte added.
        for keep in [0, 1, 12, length - 1] {
            assert_eq!(exchange_changing(which, &|m| m[..keep].to_vec()), Err(PairError::Failed), "{name} cut to {keep}");
        }
        assert_eq!(exchange_changing(which, &|m| [m, &[0u8][..]].concat()), Err(PairError::Failed), "{name} with a byte added");
    }
}

#[test]
fn messages_from_a_finished_exchange_cannot_be_used_again() {
    let fingerprint = hub_certificate();
    let (_, old) = exchange("482913", "482913", &fingerprint, &fingerprint).unwrap();

    // Someone who recorded a device's messages plays them to the hub again,
    // while the same code is still showing. The hub's part is new every time,
    // so the old confirmation no longer fits.
    let (hub, _) = Responder::start(&code("482913"), &old.device_name, &old.device_hello, &fingerprint).unwrap();
    assert!(matches!(hub.verify(&old.device_confirmation), Err(PairError::Failed)));

    // Someone who recorded a hub's messages plays them to a device again.
    let device = Initiator::new(&code("482913"), "Galaxy Tab S9+").unwrap();
    let (device, _) = device.finish(&old.hub_hello, &fingerprint).unwrap();
    assert!(matches!(device.accept(&old.hub_confirmation, &old.sealed_grant), Err(PairError::Failed)));
}

#[test]
fn a_message_sent_back_to_its_own_side_is_refused() {
    let fingerprint = hub_certificate();
    // A device handed its own first message as if it were the hub's.
    let device = Initiator::new(&code("482913"), "tablet").unwrap();
    let own = device.hello().to_vec();
    assert!(matches!(device.finish(&own, &fingerprint), Err(PairError::Failed)));

    // A hub handed another hub's message as if it came from a device.
    let device = Initiator::new(&code("482913"), "tablet").unwrap();
    let (_, hub_hello) = Responder::start(&code("482913"), "tablet", device.hello(), &fingerprint).unwrap();
    assert!(matches!(Responder::start(&code("482913"), "tablet", &hub_hello, &fingerprint), Err(PairError::Failed)));

    // Messages of the wrong length are refused as well.
    for bad in [&b""[..], &b"A"[..], &[0x41; 32][..], &[0x41; 64][..]] {
        assert!(matches!(Responder::start(&code("482913"), "tablet", bad, &fingerprint), Err(PairError::Failed)));
        let device = Initiator::new(&code("482913"), "tablet").unwrap();
        assert!(matches!(device.finish(bad, &fingerprint), Err(PairError::Failed)));
    }
}

#[test]
fn a_wrong_code_and_a_changed_message_look_the_same() {
    let fingerprint = hub_certificate();
    let wrong_code = exchange("000001", "000002", &fingerprint, &fingerprint).err().unwrap();
    let wrong_certificate = exchange("000001", "000001", &certificate_fingerprint(b"x"), &fingerprint).err().unwrap();
    let changed = {
        let device = Initiator::new(&code("000001"), "tablet").unwrap();
        let (hub, hub_hello) = Responder::start(&code("000001"), "tablet", device.hello(), &fingerprint).unwrap();
        let (_, confirmation) = device.finish(&hub_hello, &fingerprint).unwrap();
        hub.verify(&flipped(&confirmation, 0)).err().unwrap()
    };
    assert_eq!(wrong_code, PairError::Failed);
    assert_eq!(wrong_certificate, wrong_code);
    assert_eq!(changed, wrong_code);
    assert_eq!(changed.to_string(), wrong_code.to_string());
}

#[test]
fn every_code_but_the_right_one_fails() {
    // A sample of the million codes, the neighbours of the right one included.
    let fingerprint = hub_certificate();
    for guess in ["482912", "482914", "482903", "382913", "000000", "999999", "319284"] {
        assert!(matches!(exchange(guess, "482913", &fingerprint, &fingerprint), Err(PairError::Failed)), "{guess}");
    }
}
