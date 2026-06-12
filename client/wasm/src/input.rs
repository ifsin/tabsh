pub fn encode_key(
    key: &str,
    code: &str,
    shift: bool,
    ctrl: bool,
    alt: bool,
    meta: bool,
) -> Vec<u8> {
    if ctrl {
        if code == "Backspace" {
            return b"\x1b\x7f".to_vec();
        }
        let b = ctrl_byte(key, code);
        if b != 0 {
            return if alt { vec![0x1b, b] } else { vec![b] };
        }
    }

    if meta {
        return match code {
            "ArrowLeft" => vec![0x01],
            "ArrowRight" => vec![0x05],
            "Backspace" => vec![0x15],
            _ => Vec::new(),
        };
    }

    if alt {
        match code {
            "ArrowLeft" => return b"\x1bb".to_vec(),
            "ArrowRight" => return b"\x1bf".to_vec(),
            "Backspace" => return b"\x1b\x7f".to_vec(),
            _ => {}
        }
        let mut inner = encode_key(key, code, shift, false, false, meta);
        if !inner.is_empty() {
            inner.insert(0, 0x1b);
            return inner;
        }
    }

    let seq: &[u8] = match code {
        "ArrowUp" => return csi(shift, ctrl, 'A'),
        "ArrowDown" => return csi(shift, ctrl, 'B'),
        "ArrowRight" => return csi(shift, ctrl, 'C'),
        "ArrowLeft" => return csi(shift, ctrl, 'D'),
        "Home" => b"\x1b[H",
        "End" => b"\x1b[F",
        "Insert" => b"\x1b[2~",
        "Delete" => b"\x1b[3~",
        "PageUp" => b"\x1b[5~",
        "PageDown" => b"\x1b[6~",
        "F1" => b"\x1bOP",
        "F2" => b"\x1bOQ",
        "F3" => b"\x1bOR",
        "F4" => b"\x1bOS",
        "F5" => b"\x1b[15~",
        "F6" => b"\x1b[17~",
        "F7" => b"\x1b[18~",
        "F8" => b"\x1b[19~",
        "F9" => b"\x1b[20~",
        "F10" => b"\x1b[21~",
        "F11" => b"\x1b[23~",
        "F12" => b"\x1b[24~",
        "Backspace" => b"\x7f",
        "Tab" => {
            return if shift {
                b"\x1b[Z".to_vec()
            } else {
                b"\t".to_vec()
            }
        }
        "Enter" => b"\r",
        "Escape" => b"\x1b",
        _ => b"",
    };
    if !seq.is_empty() {
        return seq.to_vec();
    }

    if key.chars().count() == 1 {
        return key.as_bytes().to_vec();
    }

    Vec::new()
}

fn ctrl_byte(key: &str, code: &str) -> u8 {
    let ch = if key.len() == 1 {
        key.chars().next().unwrap()
    } else if code.starts_with("Key") && code.len() == 4 {
        code.chars().nth(3).unwrap()
    } else {
        return 0;
    };
    match ch.to_ascii_uppercase() {
        c @ 'A'..='_' => c as u8 - b'@',
        _ => 0,
    }
}

fn csi(shift: bool, ctrl: bool, dir: char) -> Vec<u8> {
    match (shift, ctrl) {
        (false, false) => vec![0x1b, b'[', dir as u8],
        (true, false) => vec![0x1b, b'[', b'1', b';', b'2', dir as u8],
        (false, true) => vec![0x1b, b'[', b'1', b';', b'5', dir as u8],
        (true, true) => vec![0x1b, b'[', b'1', b';', b'6', dir as u8],
    }
}
