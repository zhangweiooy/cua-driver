//! Pure event-tap normalization. No AX, AppKit or WindowServer calls belong here.
use cua_driver_core::teaching::{Counters, Key, Kind, Raw};
use std::sync::{atomic::Ordering, mpsc::SyncSender};

/// Preserve the event's binding; the consumer may verify it, never replace it.
pub(super) fn enqueue(
    tx: &SyncSender<Raw>,
    counters: &Counters,
    kind: Kind,
    window: u64,
    pid: u32,
    at: u64,
) {
    if tx
        .try_send(Raw {
            window,
            focus: window,
            pid,
            at,
            kind,
        })
        .is_err()
    {
        counters.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

/// A current observation is evidence only when it matches the original event.
pub(super) fn verify_input(
    event: Raw,
    sampled_at: u64,
    current: impl FnOnce() -> Option<Raw>,
) -> Option<Raw> {
    if event.window == 0 || event.pid == 0 || sampled_at.saturating_sub(event.at) > 250 {
        return None;
    }
    let current = current()?;
    if (current.window, current.pid) != (event.window, event.pid)
        || current.at.saturating_sub(event.at) > 250
    {
        return None;
    }
    Some(event)
}

// Native key codes are confined to this adapter. Letter keys used as shortcuts
// are normalized; other printable keys only trigger an AX value observation.
pub(super) fn key(code: i64, modifiers: u8) -> Key {
    if code == 9 && modifiers & 8 != 0 {
        return Key::Paste;
    }
    let letter = match code {
        0 => 'A',
        1 => 'S',
        2 => 'D',
        3 => 'F',
        4 => 'H',
        5 => 'G',
        6 => 'Z',
        7 => 'X',
        8 => 'C',
        9 => 'V',
        11 => 'B',
        12 => 'Q',
        13 => 'W',
        14 => 'E',
        15 => 'R',
        16 => 'Y',
        17 => 'T',
        31 => 'O',
        32 => 'U',
        34 => 'I',
        35 => 'P',
        37 => 'L',
        38 => 'J',
        40 => 'K',
        45 => 'N',
        46 => 'M',
        18 => '1',
        19 => '2',
        20 => '3',
        21 => '4',
        23 => '5',
        22 => '6',
        26 => '7',
        28 => '8',
        25 => '9',
        29 => '0',
        49 => ' ',
        _ => '\0',
    };
    if letter != '\0' {
        return if modifiers & 14 == 4 {
            Key::TextInput
        } else {
            Key::Character(letter)
        };
    }
    match code {
        36 | 76 => Key::Named("Enter"),
        48 => Key::Named("Tab"),
        51 => Key::Named("Backspace"),
        53 => Key::Named("Escape"),
        54..=63 => Key::Modifier,
        115 => Key::Named("Home"),
        116 => Key::Named("PageUp"),
        117 => Key::Named("Delete"),
        119 => Key::Named("End"),
        121 => Key::Named("PageDown"),
        123 => Key::Named("Left"),
        124 => Key::Named("Right"),
        125 => Key::Named("Down"),
        126 => Key::Named("Up"),
        122 => Key::Named("F1"),
        120 => Key::Named("F2"),
        99 => Key::Named("F3"),
        118 => Key::Named("F4"),
        96 => Key::Named("F5"),
        97 => Key::Named("F6"),
        98 => Key::Named("F7"),
        100 => Key::Named("F8"),
        101 => Key::Named("F9"),
        109 => Key::Named("F10"),
        103 => Key::Named("F11"),
        111 => Key::Named("F12"),
        _ if modifiers & 14 == 4 => Key::TextInput,
        _ => Key::Text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn option_text_and_dead_keys_are_distinct_from_shortcuts() {
        for modifiers in [4, 5] {
            assert_eq!(key(9, modifiers), Key::TextInput); // Option+V / Shift+Option+V
            assert_eq!(key(14, modifiers), Key::TextInput); // Option+E dead key
            assert_eq!(key(42, modifiers), Key::TextInput); // punctuation
            assert_eq!(key(123, modifiers), Key::Named("Left"));
        }
        assert_eq!(key(9, 0), Key::Character('V'));
        assert_eq!(key(9, 8), Key::Paste);
        assert_eq!(key(0, 12), Key::Character('A')); // Command+Option shortcut
        assert_eq!(key(0, 6), Key::Character('A')); // Control+Option shortcut
    }

    #[test]
    fn callback_queue_is_bounded_and_preserves_event_time_without_identity_lookup() {
        let (tx, rx) = mpsc::sync_channel(256);
        let counters = Counters::default();
        // No consumer runs during this burst. Full/disconnected queues must
        // return immediately, without waiting for native context work.
        for at in 0..512 {
            enqueue(&tx, &counters, Kind::Key(key(9, 4), 4), 10, 123, at);
        }
        assert_eq!(counters.dropped.load(Ordering::Relaxed), 256);
        for at in 0..256 {
            let raw = rx.try_recv().unwrap();
            assert_eq!((raw.window, raw.focus, raw.pid, raw.at), (10, 10, 123, at));
            assert!(matches!(raw.kind, Kind::Key(Key::TextInput, 4)));
        }
        assert!(rx.try_recv().is_err());
        drop(rx);
        enqueue(&tx, &counters, Kind::Focus, 10, 123, 512);
        assert_eq!(counters.dropped.load(Ordering::Relaxed), 257);
    }

    #[test]
    fn delayed_input_never_rebinds_to_another_window_of_the_same_app() {
        for kind in [
            Kind::Key(key(9, 4), 4),
            Kind::Click(30, 40, "left"),
            Kind::Scroll(30, 40, 120, false),
        ] {
            let original = Raw {
                window: 10,
                focus: 10,
                pid: 123,
                at: 100,
                kind,
            };
            // A -> B focus switch, or closing A reveals B at the same point.
            let other = Raw {
                window: 20,
                focus: 20,
                at: 110,
                ..original
            };
            assert!(verify_input(original, 110, || Some(other)).is_none());
            assert!(verify_input(original, 110, || None).is_none());
            assert!(verify_input(original, 110, || Some(Raw {
                pid: 456,
                ..original
            }))
            .is_none());
            let retained = verify_input(original, 110, || {
                Some(Raw {
                    at: 110,
                    ..original
                })
            })
            .unwrap();
            assert_eq!(
                (retained.window, retained.pid, retained.focus, retained.at),
                (10, 123, 10, 100)
            );
            assert!(verify_input(original, 110, || Some(Raw {
                at: 351,
                ..original
            }))
            .is_none());
            for unverified in [
                Raw {
                    window: 0,
                    ..original
                },
                Raw { pid: 0, ..original },
                Raw { at: 0, ..original },
            ] {
                assert!(verify_input(unverified, 251, || panic!(
                    "unbound/stale input must not query today's window"
                ))
                .is_none());
            }
        }
    }
}
