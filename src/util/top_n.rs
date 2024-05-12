use std::collections::BTreeMap;

pub struct TopN<K: Ord, V> {
    limit: Option<u32>,
    count: u32,
    echelons: BTreeMap<K, Vec<V>>,
}

impl<K: Ord, V> TopN<K, V> {
    pub fn new(limit: u32) -> TopN<K, V> {
        debug_assert_ne!(limit, 0);
        TopN {
            limit: Some(limit),
            count: 0,
            echelons: BTreeMap::new(),
        }
    }

    pub fn limitless() -> TopN<K, V> {
        TopN {
            limit: None,
            count: 0,
            echelons: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, k: K, v: V) -> Option<V>
    where
        K: Clone,
    {
        self.count += 1;
        self.echelons.entry(k).or_default().push(v);

        if let Some(limit) = self.limit {
            if limit < self.count {
                self.count -= 1;

                let last_key = self.echelons.iter().next_back().unwrap().0.clone();

                let mut last_echelon = self.echelons.remove(&last_key).unwrap();
                let popped = last_echelon.pop().unwrap();
                if !last_echelon.is_empty() {
                    self.echelons.insert(last_key, last_echelon);
                }
                return Some(popped);
            }
        }
        None
    }

    // see: https://github.com/rust-lang/rfcs/blob/master/text/1522-conservative-impl-trait.md
    //    pub fn values(&self) -> impl Iterator<Item=&V> {
    //        self.echelons.values().flat_map(|v| v)
    //    }
    pub fn values(&self) -> Vec<V>
    where
        V: Clone,
    {
        self.echelons
            .values()
            .flat_map(|v| v.iter().cloned())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_one() {
        let mut top_n = TopN::new(5);
        top_n.insert("asdf", 1);
    }

    #[test]
    fn test_insert_to_limit() {
        let mut top_n = TopN::new(2);
        top_n.insert("asdf", 1);
        top_n.insert("xyz", 2);
    }

    #[test]
    fn test_insert_past_limit_bigger_discarded() {
        let mut top_n = TopN::new(2);
        top_n.insert("a", 1);
        top_n.insert("b", 2);
        top_n.insert("z", -1);
        assert_eq!(top_n.values(), vec![1, 2]);
    }

    #[test]
    fn test_insert_past_limit_equal_discarded() {
        let mut top_n = TopN::new(2);
        top_n.insert("a", 1);
        top_n.insert("b", 2);
        top_n.insert("b", -1);
        assert_eq!(top_n.values(), vec![1, 2]);
    }

    #[test]
    fn test_insert_past_limit_smaller_last_one_discarded() {
        let mut top_n = TopN::new(2);
        top_n.insert("b", "second");
        top_n.insert("c", "last");
        top_n.insert("a", "first");
        assert_eq!(top_n.values(), vec!["first", "second"]);
    }

    #[test]
    fn test_insert_past_limit_comprehensive() {
        let mut top_n = TopN::new(5);
        top_n.insert("asdf", 1);
        assert_eq!(top_n.values(), vec![1]);
        top_n.insert("asdf", 3);
        assert_eq!(top_n.values(), vec![1, 3]);
        top_n.insert("asdf", 3);
        assert_eq!(top_n.values(), vec![1, 3, 3]);
        top_n.insert("xyz", 4);
        assert_eq!(top_n.values(), vec![1, 3, 3, 4]);
        top_n.insert("asdf", 2);
        assert_eq!(top_n.values(), vec![1, 3, 3, 2, 4]);
        top_n.insert("xyz", 5);
        assert_eq!(top_n.values(), vec![1, 3, 3, 2, 4]);
        top_n.insert("asdf", -1);
        assert_eq!(top_n.values(), vec![1, 3, 3, 2, -1]);
    }

    #[test]
    fn test_limitless() {
        let mut top_n = TopN::limitless();
        top_n.insert("z", 3);
        top_n.insert("y", 2);
        top_n.insert("a", 1);
        top_n.insert("a", 0);
        assert_eq!(top_n.values(), vec![1, 0, 2, 3]);
    }
}
