use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

pub trait HashValue {
    fn hash_value(&self) -> u64;
}

impl<T> HashValue for T
where
    T: Hash,
{
    fn hash_value(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish()
    }
}
