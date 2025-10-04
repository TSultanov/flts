use std::marker::PhantomData;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct VecSlice<T> {
    pub start: usize,
    pub len: usize,
    phantom: PhantomData<T>,
}

impl<T> VecSlice<T> {
    pub fn empty() -> VecSlice<T> {
        VecSlice {
            start: 0,
            len: 0,
            phantom: PhantomData,
        }
    }

    pub fn new(start: usize, len: usize) -> VecSlice<T> {
        VecSlice {
            start,
            len,
            phantom: PhantomData,
        }
    }

    pub fn end(&self) -> usize {
        self.start + self.len
    }

    pub fn slice<'a>(&self, source: &'a [T]) -> &'a [T] {
        &source[self.start..self.end()]
    }
}

pub fn push_string(strings: &mut Vec<u8>, string: &str) -> VecSlice<u8> {
    let start = strings.len();
    strings.extend(string.bytes());
    VecSlice {
        start,
        len: string.len(),
        phantom: PhantomData,
    }
}

pub fn push<T: Clone>(items: &mut Vec<T>, slice: &VecSlice<T>, item: T) -> Option<VecSlice<T>> {
    let mut slice = slice.clone();
    if slice.end() > items.len() {
        return None;
    }

    if slice.end() < items.len() {
        let slice_items_copy = slice.slice(items).to_vec();
        slice.start = items.len();
        items.extend(slice_items_copy);
    }

    items.push(item);
    slice.len += 1;
    Some(slice)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_empty() {
        let mut vec = vec![];
        let slice = VecSlice {
            start: 0,
            len: 0,
            phantom: PhantomData,
        };
        let slice = push(&mut vec, &slice, 4).unwrap();
        let slice = slice.slice(&vec);
        assert_eq!(slice, vec![4]);
        assert_eq!(vec, vec![4]);
    }

    #[test]
    fn push_trivial() {
        let mut vec = vec![1, 2, 3];
        let slice = VecSlice {
            start: 1,
            len: 2,
            phantom: PhantomData,
        };
        let slice = push(&mut vec, &slice, 4).unwrap();
        let slice = slice.slice(&vec);
        assert_eq!(slice, vec![2, 3, 4]);
        assert_eq!(vec, vec![1, 2, 3, 4]);
    }

    #[test]
    fn push_beginning() {
        let mut vec = vec![1, 2, 3];
        let slice = VecSlice {
            start: 0,
            len: 1,
            phantom: PhantomData,
        };
        let slice = push(&mut vec, &slice, 4).unwrap();
        let slice = slice.slice(&vec);
        assert_eq!(slice, vec![1, 4]);
        assert_eq!(vec, vec![1, 2, 3, 1, 4]);
    }

    #[test]
    fn push_just_at_end() {
        let mut vec = vec![1, 2, 3];
        let slice = VecSlice {
            start: 2,
            len: 1,
            phantom: PhantomData,
        };
        let slice = push(&mut vec, &slice, 4).unwrap();
        let slice = slice.slice(&vec);
        assert_eq!(slice, vec![3, 4]);
        assert_eq!(vec, vec![1, 2, 3, 4]);
    }

    #[test]
    fn push_outside() {
        let mut vec = vec![1, 2, 3];
        let slice = VecSlice {
            start: 3,
            len: 1,
            phantom: PhantomData,
        };
        let slice = push(&mut vec, &slice, 4);
        assert_eq!(slice, None);
        assert_eq!(vec, vec![1, 2, 3]);
    }
}
