/// Append without growing the buffer past the byte budget. Keep geometric
/// growth below the ceiling so reading in small chunks stays amortized.
pub(crate) fn append(buf: &mut Vec<u8>, bytes: &[u8], limit: usize) -> bool {
    if bytes.len() > limit - buf.len() {
        return false;
    }
    let needed = buf.len() + bytes.len();
    if needed > buf.capacity() {
        let target = needed.max(buf.capacity().saturating_mul(2).min(limit));
        buf.reserve_exact(target - buf.len());
    }
    buf.extend_from_slice(bytes);
    true
}
