use std::cell::RefCell;
use std::rc::Rc;

/// Callbacks that run when a component changes. Everything runs on the main thread, so slots may change other
/// components; a slot may also connect more slots, which run from the next emission.
#[derive(Default)]
pub struct Signal {
    slots: RefCell<Vec<Rc<dyn Fn()>>>,
}

impl Signal {
    pub fn connect(&self, slot: impl Fn() + 'static) {
        self.slots.borrow_mut().push(Rc::new(slot));
    }

    pub fn emit(&self) {
        // Slots run without the list borrowed, so they can emit or connect again.
        let slots = self.slots.borrow().clone();
        for slot in slots {
            slot();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Signal;
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn slots_run_in_order_and_may_connect_more() {
        let signal = Rc::new(Signal::default());
        let calls = Rc::new(Cell::new(0));
        let (inner, counter) = (signal.clone(), calls.clone());
        signal.connect(move || {
            counter.set(counter.get() + 1);
            let counter = counter.clone();
            inner.connect(move || counter.set(counter.get() + 10));
        });
        signal.emit();
        assert_eq!(calls.get(), 1);
        signal.emit();
        assert_eq!(calls.get(), 12);
    }
}
