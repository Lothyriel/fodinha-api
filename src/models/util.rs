#[derive(Debug)]
pub struct CyclicIterator {
    items: Vec<usize>,
    current_index: usize,
}

impl CyclicIterator {
    pub fn new(count: usize) -> Self {
        CyclicIterator {
            items: (0..count).collect(),
            current_index: 0,
        }
    }

    pub fn shift(&mut self) {
        self.reset();

        if !self.items.is_empty() {
            self.items.rotate_left(1);
        }
    }

    pub fn shift_to(&mut self, item: usize) -> Option<()> {
        self.reset();
        let idx = self.items.iter().position(|&i| i == item)?;
        let n = self.items.len();
        self.items.rotate_right(n - idx);
        Some(())
    }

    pub fn peek(&self) -> Option<usize> {
        self.items.get(self.current_index).copied()
    }

    pub fn peek_next(&self) -> Option<usize> {
        self.items.get(self.current_index + 1).copied()
    }

    pub fn remove(&mut self, item: usize) -> Option<usize> {
        let idx = self.items.iter().position(|&i| i == item)?;
        self.items.remove(idx);
        Some(idx)
    }

    fn reset(&mut self) {
        self.current_index = 0;
    }
}

impl Iterator for CyclicIterator {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.items.get(self.current_index).copied();

        self.current_index += 1;

        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shift() {
        let mut cyclic = CyclicIterator::new(5);

        let result: Vec<_> = cyclic.by_ref().collect();
        assert_eq!(result, vec![0, 1, 2, 3, 4]);

        cyclic.shift();
        let result: Vec<_> = cyclic.by_ref().collect();
        assert_eq!(result, vec![1, 2, 3, 4, 0]);

        cyclic.shift();
        let result: Vec<_> = cyclic.by_ref().collect();
        assert_eq!(result, vec![2, 3, 4, 0, 1]);

        cyclic.reset();
        let result: Vec<_> = cyclic.by_ref().collect();
        assert_eq!(result, vec![2, 3, 4, 0, 1]);
    }

    #[test]
    fn test_shift_to() {
        let mut cyclic = CyclicIterator::new(5);

        let result: Vec<_> = cyclic.by_ref().collect();
        assert_eq!(result, vec![0, 1, 2, 3, 4]);

        cyclic.shift_to(4);
        let result: Vec<_> = cyclic.by_ref().collect();
        assert_eq!(result, vec![4, 0, 1, 2, 3]);

        cyclic.shift_to(2);
        let result: Vec<_> = cyclic.by_ref().collect();
        assert_eq!(result, vec![2, 3, 4, 0, 1]);

        cyclic.shift_to(3);
        let result: Vec<_> = cyclic.by_ref().collect();
        assert_eq!(result, vec![3, 4, 0, 1, 2]);
    }

    #[test]
    fn test_single_element() {
        let mut cyclic = CyclicIterator::new(1);

        assert_eq!(cyclic.next(), Some(0));
        assert_eq!(cyclic.next(), None);
    }

    #[test]
    fn test_empty() {
        let mut cyclic = CyclicIterator::new(0);

        assert_eq!(cyclic.next(), None);

        cyclic.reset();

        assert_eq!(cyclic.next(), None);
    }

    #[test]
    fn test_remove() {
        //[0,1,2,3,4]
        let mut cyclic = CyclicIterator::new(5);

        //[1,2,3,4,0]
        cyclic.shift();
        //[2,3,4,0,1]
        cyclic.shift();

        let result: Vec<_> = cyclic.by_ref().collect();
        assert_eq!(result, vec![2, 3, 4, 0, 1]);
        cyclic.reset();

        //[2,3,4,1]
        cyclic.remove(0);
        let result: Vec<_> = cyclic.by_ref().collect();
        assert_eq!(result, vec![2, 3, 4, 1]);
        cyclic.reset();

        //[2,3,1]
        cyclic.remove(4);
        let result: Vec<_> = cyclic.by_ref().collect();
        assert_eq!(result, vec![2, 3, 1]);
        cyclic.reset();

        //[3,1]
        cyclic.remove(2);
        let result: Vec<_> = cyclic.by_ref().collect();
        assert_eq!(result, vec![3, 1]);
        cyclic.reset();
    }
}
