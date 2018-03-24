mod top_n;

use std::cmp::Ordering;
use std::error::Error;
use std::io;
use std::path::Path;
use std::rc::Rc;

use term;
use term::StdoutTerminal;

pub use self::top_n::TopN;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd)]
pub struct Criteria<T> {
    /// Values of current row to sort with, placed in order of significance.
    values: Vec<T>,
    /// Shared smart reference to Vector of boolean where each index corresponds to whether the
    /// field at that index should be ordered in ascending order `true` or descending order `false`.
    orderings: Rc<Vec<bool>>,
}

impl<T> Criteria<T> {
    pub fn new(values: Vec<T>, orderings: Rc<Vec<bool>>) -> Criteria<T> {
        debug_assert_eq!(values.len(), orderings.len());
        Criteria { values, orderings }
    }
    #[inline]
    fn cmp_at(&self, other: &Self, i: usize) -> Ordering where T: Ord {
        let comparison = self.cmp_at_direct(other, i);
        if self.orderings[i] { comparison } else { comparison.reverse() }
    }
    #[inline]
    fn cmp_at_direct(&self, other: &Self, i: usize) -> Ordering where T: Ord {
        if self.values[i] < other.values[i] {
            Ordering::Less
        } else if self.values[i] > other.values[i] {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    }
}

impl<T: Ord> Ord for Criteria<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        for i in 0..(self.values.len().min(other.values.len())) {
            let ord = self.cmp_at(other, i);
            if ord != Ordering::Equal {
                return ord;
            }
        }

        self.values.len().cmp(&other.values.len())
    }
}

pub fn path_error_message(p: &Path, e: io::Error, t: &mut Box<StdoutTerminal>) {
    error_message(&p.to_string_lossy(), e.description(), t);
}

pub fn error_message(source: &str, description: &str, t: &mut Box<StdoutTerminal>) {
    t.fg(term::color::YELLOW).unwrap();
    eprint!("{}", source);
    t.reset().unwrap();

    eprint!(": ");

    t.fg(term::color::RED).unwrap();
    eprintln!("{}", description);
    t.reset().unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn basic_criteria<T: Ord + Clone>(vals: &[T]) -> Criteria<T> {
        let orderings = Rc::new(vec![true; vals.len()]);
        Criteria::new(vals.to_vec(), orderings)
    }

    #[test]
    fn test_compare_same() {
        let c1 = basic_criteria(&[1, 2, 3]);
        let c2 = basic_criteria(&[1, 2, 3]);
        assert_eq!(c1.cmp(&c2), Ordering::Equal);
    }

    #[test]
    fn test_compare_first_smaller() {
        let c1 = basic_criteria(&[1, 2, 3]);
        let c2 = basic_criteria(&[3, 2, 3]);
        assert_eq!(c1.cmp(&c2), Ordering::Less);
    }

    #[test]
    fn test_compare_first_smaller_same_prefix() {
        let c1 = basic_criteria(&[1, 2, 3]);
        let c2 = basic_criteria(&[1, 3, 3]);
        assert_eq!(c1.cmp(&c2), Ordering::Less);
    }

    #[test]
    fn test_compare_shorter_smaller_same_prefix() {
        let c1 = basic_criteria(&[1, 2, 3]);
        let c2 = basic_criteria(&[1, 2, 3, 4]);
        assert_eq!(c1.cmp(&c2), Ordering::Less);
    }

    #[test]
    fn test_compare_all_fields_reverse() {
        let orderings = Rc::new(vec![false, false, false]);
        let c1 = Criteria::new(vec![1, 2, 3], orderings.clone());
        let c2 = Criteria::new(vec![1, 3, 1], orderings.clone());
        assert_eq!(c1.cmp(&c2), Ordering::Greater);
    }

    #[test]
    fn test_compare_some_fields_reverse() {
        let orderings = Rc::new(vec![true, false, true]);
        let c1 = Criteria::new(vec![1, 2, 3], orderings.clone());
        let c2 = Criteria::new(vec![1, 3, 1], orderings.clone());
        assert_eq!(c1.cmp(&c2), Ordering::Greater);
    }

}
