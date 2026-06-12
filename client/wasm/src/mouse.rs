pub const MODE_SGR: u32 = 1 << 5;

const KIND_RELEASE: u8 = 1;
const KIND_MOTION: u8 = 2;
const KIND_WHEEL_UP: u8 = 3;
const KIND_WHEEL_DOWN: u8 = 4;

pub fn encode_mouse(
    kind: u8,
    button: u8,
    col: u16,
    row: u16,
    shift: bool,
    alt: bool,
    ctrl: bool,
    sgr: bool,
) -> Vec<u8> {
    let mut cb: u32 = match kind {
        KIND_WHEEL_UP => 64,
        KIND_WHEEL_DOWN => 65,
        KIND_MOTION => button as u32 + 32,
        _ => button as u32,
    };
    if shift {
        cb += 4;
    }
    if alt {
        cb += 8;
    }
    if ctrl {
        cb += 16;
    }

    let x = col + 1;
    let y = row + 1;

    if sgr {
        let trailer = if kind == KIND_RELEASE { 'm' } else { 'M' };
        return format!("\x1b[<{cb};{x};{y}{trailer}").into_bytes();
    }

    let cb_legacy = if kind == KIND_RELEASE {
        3 + (cb & !3)
    } else {
        cb
    };
    let bx = (x.min(223) as u8).wrapping_add(32);
    let by = (y.min(223) as u8).wrapping_add(32);
    vec![0x1b, b'[', b'M', (cb_legacy as u8).wrapping_add(32), bx, by]
}
