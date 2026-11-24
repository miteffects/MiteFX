//! Parameter ids for indexed parameters (`b2_gain`, `t3_time`), built without allocating: the
//! audio thread reads parameters by id when it turns them into settings, and must not allocate.

use std::borrow::Borrow;
use std::fmt;
use std::ops::Deref;

/// Longest id it holds (the longest real one is under 24 bytes).
const CAP: usize = 40;

/// An id like `b2_gain`, held inline. Reads as a `&str`; `String::from` makes an owned copy.
#[derive(Clone, Copy)]
pub struct ParamId {
    buf: [u8; CAP],
    len: u8,
}

impl ParamId {
    /// `{prefix}{n}_{param}`, the number zero-padded to `width` digits (`b01_freq` for width 2).
    pub fn indexed(prefix: &str, n: usize, width: usize, param: &str) -> Self {
        let mut id = Self { buf: [0; CAP], len: 0 };
        id.push(prefix);
        let mut digits = [0u8; 20];
        let mut count = 0;
        let mut v = n;
        loop {
            digits[count] = b'0' + (v % 10) as u8;
            count += 1;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        for _ in count..width {
            id.push("0");
        }
        for k in (0..count).rev() {
            id.push_byte(digits[k]);
        }
        id.push("_");
        id.push(param);
        id
    }

    fn push(&mut self, s: &str) {
        for &b in s.as_bytes() {
            self.push_byte(b);
        }
    }

    fn push_byte(&mut self, b: u8) {
        let at = self.len as usize;
        assert!(at < CAP, "parameter id longer than {CAP} bytes");
        self.buf[at] = b;
        self.len += 1;
    }

    pub fn as_str(&self) -> &str {
        // Built only from `&str`s and ASCII digits, never cut inside a character.
        std::str::from_utf8(&self.buf[..self.len as usize]).expect("ids are UTF-8")
    }
}

impl Deref for ParamId {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for ParamId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for ParamId {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for ParamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for ParamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl PartialEq for ParamId {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}
impl Eq for ParamId {}

impl PartialEq<str> for ParamId {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for ParamId {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<String> for ParamId {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<ParamId> for String {
    fn eq(&self, other: &ParamId) -> bool {
        self.as_str() == other.as_str()
    }
}

impl PartialEq<ParamId> for str {
    fn eq(&self, other: &ParamId) -> bool {
        self == other.as_str()
    }
}

impl PartialEq<ParamId> for &str {
    fn eq(&self, other: &ParamId) -> bool {
        *self == other.as_str()
    }
}

impl std::hash::Hash for ParamId {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

impl From<ParamId> for String {
    fn from(id: ParamId) -> String {
        id.as_str().to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_ids_format_would() {
        assert_eq!(ParamId::indexed("b", 2, 0, "gain").as_str(), "b2_gain");
        assert_eq!(ParamId::indexed("b", 3, 2, "freq").as_str(), "b03_freq");
        assert_eq!(ParamId::indexed("b", 12, 2, "q").as_str(), "b12_q");
        assert_eq!(ParamId::indexed("lfo", 10, 0, "rate").as_str(), "lfo10_rate");
        assert_eq!(&*ParamId::indexed("t", 1, 0, "time"), "t1_time");
        assert_eq!(String::from(ParamId::indexed("k", 4, 0, "on")), "k4_on");
    }
}
