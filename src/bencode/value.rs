use std::collections::BTreeMap;
use std::fmt;

/// Represents a bencoded value.
#[derive(Clone, PartialEq, Eq)]
pub enum BValue {
    /// Integer: "i42e" -> Int(42)
    Int(i64),
    /// Byte string: "4:spam" -> Bytes(b"spam")
    Bytes(Vec<u8>),
    /// List: "l4:spami42ee" -> List([Bytes("spam"), Int(42)])
    List(Vec<BValue>),
    /// Dictionary: keys must be sorted byte strings.
    /// Uses BTreeMap for automatic sorted order (required for info_hash computation).
    Dict(BTreeMap<Vec<u8>, BValue>),
}

impl BValue {
    /// Try to interpret this value as an integer.
    pub fn as_int(&self) -> Option<i64> {
        match self {
            BValue::Int(n) => Some(*n),
            _ => None,
        }
    }

    /// Try to interpret this value as a byte string.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            BValue::Bytes(b) => Some(b),
            _ => None,
        }
    }

    /// Try to interpret this value as a UTF-8 string.
    pub fn as_str(&self) -> Option<&str> {
        self.as_bytes().and_then(|b| std::str::from_utf8(b).ok())
    }

    /// Try to interpret this value as a list.
    pub fn as_list(&self) -> Option<&[BValue]> {
        match self {
            BValue::List(l) => Some(l),
            _ => None,
        }
    }

    /// Try to interpret this value as a dictionary.
    pub fn as_dict(&self) -> Option<&BTreeMap<Vec<u8>, BValue>> {
        match self {
            BValue::Dict(d) => Some(d),
            _ => None,
        }
    }

    /// Look up a key in this value (if it's a dictionary).
    pub fn get(&self, key: &[u8]) -> Option<&BValue> {
        self.as_dict().and_then(|d| d.get(key))
    }

    /// Convenience: look up a key by str.
    pub fn get_str(&self, key: &str) -> Option<&BValue> {
        self.get(key.as_bytes())
    }
}

impl fmt::Debug for BValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BValue::Int(n) => write!(f, "Int({n})"),
            BValue::Bytes(b) => {
                if let Ok(s) = std::str::from_utf8(b) {
                    if s.len() <= 100 {
                        write!(f, "Bytes({s:?})")
                    } else {
                        write!(f, "Bytes({:?}...[{} bytes])", &s[..50], b.len())
                    }
                } else {
                    write!(f, "Bytes([{} bytes])", b.len())
                }
            }
            BValue::List(l) => f.debug_tuple("List").field(l).finish(),
            BValue::Dict(d) => {
                let mut dm = f.debug_map();
                for (k, v) in d {
                    let key = String::from_utf8_lossy(k);
                    dm.entry(&key.as_ref(), v);
                }
                dm.finish()
            }
        }
    }
}
