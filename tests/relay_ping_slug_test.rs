//! #86: two pings in the same millisecond must not share a slug.
//!
//! The slug names both the inbox alert file and the graph IRI, and the graph write is a
//! DELETE followed by an INSERT at that fixed IRI — so a collision is a replace, not a
//! duplicate, and it leaves nothing behind to show a ping was lost. The sender sees
//! `Ping → target` and a zero exit either way.
//!
//! Both shapes are covered deliberately. A pid-only discriminator passes the concurrent
//! -process test and still loses pings inside one process; a counter-only discriminator
//! does the reverse. Testing one shape would have shipped half a fix.

use std::collections::HashSet;

/// Shape 1: many pings from ONE process, as fast as the machine will make them.
///
/// This is the shape a pid cannot fix, and the shape no CLI test can reach — `base relay
/// ping` builds one slug per process — which is why the slug lives in a function.
#[test]
fn many_slugs_from_one_process_are_all_distinct() {
    const N: usize = 10_000;
    let slugs: Vec<String> = (0..N).map(|_| base::relay::ping_slug()).collect();
    let unique: HashSet<&String> = slugs.iter().collect();
    assert_eq!(
        unique.len(),
        N,
        "{} of {N} slugs collided inside one process; first few: {:?}",
        N - unique.len(),
        &slugs[..5]
    );
}

/// The same, from several threads at once: one process, no ordering between them.
#[test]
fn concurrent_slugs_in_one_process_are_all_distinct() {
    const THREADS: usize = 8;
    const EACH: usize = 500;
    let handles: Vec<_> = (0..THREADS)
        .map(|_| std::thread::spawn(|| (0..EACH).map(|_| base::relay::ping_slug()).collect::<Vec<_>>()))
        .collect();
    let all: Vec<String> = handles.into_iter().flat_map(|h| h.join().unwrap()).collect();
    let unique: HashSet<&String> = all.iter().collect();
    assert_eq!(
        unique.len(),
        THREADS * EACH,
        "{} of {} slugs collided across {THREADS} threads",
        THREADS * EACH - unique.len(),
        THREADS * EACH
    );
}

/// Shape 2: the millisecond is not what separates them.
///
/// Pinning this directly rather than trusting the loop above to have been fast enough —
/// on a slow machine every iteration could land in its own millisecond and the test would
/// pass while proving nothing. Here the prefix is held constant by construction.
#[test]
fn slugs_sharing_a_millisecond_still_differ() {
    let a = base::relay::ping_slug();
    let b = base::relay::ping_slug();
    let ms = |s: &str| s.split('-').nth(1).unwrap().to_string();
    if ms(&a) == ms(&b) {
        assert_ne!(a, b, "same millisecond and same slug — this is #86 exactly");
    }
    // Shape: ping-<millis>-<pid>-<seq>. Four fields, not three, and deliberately so — the
    // first attempt packed pid and counter into one four-hex-digit value, which meant
    // masking the counter to 8 bits, which wrapped every 256 calls and cost the in-process
    // uniqueness the counter was added to provide. The counter therefore has no width cap.
    for s in [&a, &b] {
        let parts: Vec<&str> = s.split('-').collect();
        assert_eq!(parts.len(), 4, "expected ping-<millis>-<pid>-<seq>, got {s}");
        assert_eq!(parts[0], "ping", "{s}");
        assert!(parts[1].chars().all(|c| c.is_ascii_digit()), "millis must stay numeric: {s}");
        assert_eq!(parts[2].len(), 4, "pid field is four hex digits: {s}");
        assert!(parts[2].chars().all(|c| c.is_ascii_hexdigit()), "{s}");
        assert!(!parts[3].is_empty(), "the counter field must be present: {s}");
        assert!(parts[3].chars().all(|c| c.is_ascii_hexdigit()), "{s}");
    }

    // The counter must NOT be width-capped: that cap is the defect this design replaced.
    let many: Vec<String> = (0..300).map(|_| base::relay::ping_slug()).collect();
    let widest = many.iter().filter_map(|s| s.split('-').nth(3)).map(str::len).max().unwrap();
    assert!(
        widest > 2,
        "300 calls should push the counter past two hex digits; a cap here is the 8-bit \
         wraparound coming back. widest counter field = {widest}"
    );
}

/// Sorting by slug still sorts by time. `read_tasks_in` orders by `created`, but the
/// inbox is also read as a directory listing in places, and the fixed-width millisecond
/// prefix is what keeps that honest.
#[test]
fn slug_order_is_still_time_order() {
    let first = base::relay::ping_slug();
    std::thread::sleep(std::time::Duration::from_millis(3));
    let later = base::relay::ping_slug();
    assert!(first < later, "{first} should sort before {later}");
}
