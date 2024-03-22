use std::{
    fmt,
    ops::{Deref, DerefMut},
};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum Listable<T> {
    List(Vec<T>),
    Single(T),
}

impl<T> Listable<T> {
    pub fn into_list(self) -> Vec<T> {
        match self {
            Listable::List(list) => list,
            Listable::Single(single) => vec![single],
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for Listable<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Listable::List(list) => write!(f, "List({:?})", list),
            Listable::Single(single) => write!(f, "Single({:?})", single),
        }
    }
}

impl<T: Clone> Clone for Listable<T> {
    fn clone(&self) -> Self {
        match self {
            Listable::List(list) => Listable::List(list.clone()),
            Listable::Single(single) => Listable::Single(single.clone()),
        }
    }
}

impl<T> From<Vec<T>> for Listable<T> {
    fn from(list: Vec<T>) -> Self {
        Self::List(list)
    }
}

impl<T> Default for Listable<T> {
    fn default() -> Self {
        Listable::List(Vec::new())
    }
}

impl<T> Deref for Listable<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        match self {
            Self::List(list) => list,
            Self::Single(single) => std::slice::from_ref(single),
        }
    }
}

impl<T> DerefMut for Listable<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::List(list) => list,
            Self::Single(single) => std::slice::from_mut(single),
        }
    }
}
