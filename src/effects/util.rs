use crate::speed_interval;

pub(super) fn beat_position(now_ms: u32, speed: u8, count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    if count == 1 {
        return Some(0);
    }

    let span = (count - 1) * 2;
    let step = ((now_ms / speed_interval(16, speed)) as usize) % span;
    Some(if step < count { step } else { span - step })
}
