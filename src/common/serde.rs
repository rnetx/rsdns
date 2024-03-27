use std::{
    ops::{Deref, DerefMut},
    str::FromStr,
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SingleOrList<T> {
    Single(T),
    List(Vec<T>),
}

impl<T> Default for SingleOrList<T> {
    fn default() -> Self {
        Self::List(Vec::new())
    }
}

impl<T> SingleOrList<T> {
    pub fn into_list(self) -> Vec<T> {
        match self {
            Self::Single(v) => vec![v],
            Self::List(v) => v,
        }
    }
}

impl<T> From<T> for SingleOrList<T> {
    fn from(v: T) -> Self {
        Self::Single(v)
    }
}

impl<T> From<Vec<T>> for SingleOrList<T> {
    fn from(v: Vec<T>) -> Self {
        Self::List(v)
    }
}

impl<T> Deref for SingleOrList<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Single(v) => std::slice::from_ref(v),
            Self::List(v) => v,
        }
    }
}

impl<T> DerefMut for SingleOrList<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Single(v) => std::slice::from_mut(v),
            Self::List(v) => v,
        }
    }
}

pub(crate) fn deserialize_with_from_str<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: FromStr,
    T::Err: std::fmt::Display,
{
    let s = String::deserialize(deserializer)?;
    T::from_str(&s).map_err(|e| {
        serde::de::Error::custom(format!(
            "failed to parse {}: {}",
            std::any::type_name::<T>(),
            e
        ))
    })
}

pub(crate) fn deserialize_with_option_from_str<'de, D, T>(
    deserializer: D,
) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: FromStr,
    T::Err: std::fmt::Display,
{
    let s = Option::<String>::deserialize(deserializer)?;
    match s {
        Some(s) => T::from_str(&s).map(|v| Some(v)).map_err(|e| {
            serde::de::Error::custom(format!(
                "failed to parse {}: {}",
                std::any::type_name::<T>(),
                e
            ))
        }),
        None => Ok(None),
    }
}
