use crate::error::Result;
use serde::{Serialize, de::DeserializeOwned};

pub fn serialize<T: Serialize>(obj: &T) -> Result<Vec<u8>> {
    let ser = rmp_serde::to_vec(obj)?;
    Ok(ser)
}

pub fn deserialize<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    let obj = rmp_serde::from_read(bytes)?;
    Ok(obj)
}

