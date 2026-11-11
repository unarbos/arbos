//! Main RPC child ownership. Resource effects run after the transition releases the slot lock.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BootTicket(u64);

pub(crate) enum PiSlot<T> {
    Vacant { epoch: u64 },
    Starting { ticket: BootTicket },
    Checking { ticket: BootTicket, child: T },
    Ready { ticket: BootTicket, child: T },
    Exited { ticket: BootTicket, child: T },
    Parked { epoch: u64 },
    Stopped { epoch: u64 },
}

pub(crate) enum Event<T> {
    Begin,
    Attach { ticket: BootTicket, child: T },
    Ready(BootTicket),
    Exited(BootTicket),
    Retire(BootTicket),
    Park(BootTicket),
    Abort(BootTicket),
    Shutdown,
}

pub(crate) enum Action<T> {
    Started {
        ticket: BootTicket,
        retired: Option<T>,
    },
    Accepted,
    Retired(Option<T>),
    Rejected(Option<T>),
}

impl<T> Default for PiSlot<T> {
    fn default() -> Self {
        Self::Vacant { epoch: 0 }
    }
}

impl<T> PiSlot<T> {
    fn epoch(&self) -> u64 {
        match self {
            Self::Vacant { epoch } | Self::Parked { epoch } | Self::Stopped { epoch } => *epoch,
            Self::Starting { ticket }
            | Self::Checking { ticket, .. }
            | Self::Ready { ticket, .. }
            | Self::Exited { ticket, .. } => ticket.0,
        }
    }
    pub(crate) fn ticket(&self) -> Option<BootTicket> {
        match self {
            Self::Starting { ticket }
            | Self::Checking { ticket, .. }
            | Self::Ready { ticket, .. }
            | Self::Exited { ticket, .. } => Some(*ticket),
            _ => None,
        }
    }
    fn into_child(self) -> Option<T> {
        match self {
            Self::Checking { child, .. }
            | Self::Ready { child, .. }
            | Self::Exited { child, .. } => Some(child),
            _ => None,
        }
    }
    pub(crate) fn as_ref(&self) -> Option<&T> {
        match self {
            Self::Ready { child, .. } | Self::Exited { child, .. } => Some(child),
            _ => None,
        }
    }
    pub(crate) fn current(&self) -> Option<T>
    where
        T: Clone,
    {
        self.as_ref().cloned()
    }
    pub(crate) fn is_some(&self) -> bool {
        self.as_ref().is_some()
    }
    #[cfg(test)]
    pub(crate) fn is_none(&self) -> bool {
        self.as_ref().is_none()
    }
    pub(crate) fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }
    pub(crate) fn is_booting(&self) -> bool {
        matches!(self, Self::Starting { .. } | Self::Checking { .. })
    }
    pub(crate) fn is_parked(&self) -> bool {
        matches!(self, Self::Parked { .. })
    }
    pub(crate) fn is_stopped(&self) -> bool {
        matches!(self, Self::Stopped { .. })
    }

    /// All slot writes pass here; old tickets may never retire or publish newer resources.
    pub(crate) fn transition(&mut self, event: Event<T>) -> Action<T> {
        match event {
            Event::Begin => {
                if self.is_stopped() {
                    return Action::Rejected(None);
                }
                let Some(epoch) = self.epoch().checked_add(1) else {
                    let epoch = self.epoch();
                    let old = std::mem::replace(self, Self::Stopped { epoch });
                    return Action::Rejected(old.into_child());
                };
                let ticket = BootTicket(epoch);
                let old = std::mem::replace(self, Self::Starting { ticket });
                Action::Started {
                    ticket,
                    retired: old.into_child(),
                }
            }
            Event::Attach { ticket, child } => {
                if matches!(self, Self::Starting { ticket: current } if *current == ticket) {
                    *self = Self::Checking { ticket, child };
                    Action::Accepted
                } else {
                    Action::Rejected(Some(child))
                }
            }
            Event::Ready(ticket) => {
                if !matches!(self, Self::Checking { ticket: current, .. } if *current == ticket) {
                    return Action::Rejected(None);
                }
                let epoch = self.epoch();
                let old = std::mem::replace(self, Self::Vacant { epoch });
                if let Self::Checking { child, .. } = old {
                    *self = Self::Ready { ticket, child };
                }
                Action::Accepted
            }
            Event::Exited(ticket) => {
                if !matches!(self, Self::Ready { ticket: current, .. } if *current == ticket) {
                    return Action::Rejected(None);
                }
                let epoch = self.epoch();
                let old = std::mem::replace(self, Self::Vacant { epoch });
                if let Self::Ready { child, .. } = old {
                    *self = Self::Exited { ticket, child };
                }
                Action::Accepted
            }
            Event::Retire(ticket) | Event::Park(ticket) => {
                if !matches!(self, Self::Ready { ticket: current, .. } | Self::Exited { ticket: current, .. } if *current == ticket)
                {
                    return Action::Rejected(None);
                }
                let parked = matches!(event, Event::Park(_));
                let replacement = if parked {
                    Self::Parked {
                        epoch: self.epoch(),
                    }
                } else {
                    Self::Vacant {
                        epoch: self.epoch(),
                    }
                };
                Action::Retired(std::mem::replace(self, replacement).into_child())
            }
            Event::Abort(ticket) => {
                if !matches!(self, Self::Starting { ticket: current } | Self::Checking { ticket: current, .. } if *current == ticket)
                {
                    return Action::Rejected(None);
                }
                let replacement = Self::Vacant {
                    epoch: self.epoch(),
                };
                Action::Retired(std::mem::replace(self, replacement).into_child())
            }
            Event::Shutdown => {
                let replacement = Self::Stopped {
                    epoch: self.epoch(),
                };
                Action::Retired(std::mem::replace(self, replacement).into_child())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn begin(slot: &mut PiSlot<u32>) -> BootTicket {
        match slot.transition(Event::Begin) {
            Action::Started { ticket, .. } => ticket,
            _ => panic!("begin refused"),
        }
    }
    #[test]
    fn only_the_current_checking_child_can_be_published() {
        let mut slot = PiSlot::default();
        let a = begin(&mut slot);
        let b = begin(&mut slot);
        assert!(matches!(
            slot.transition(Event::Attach {
                ticket: a,
                child: 1
            }),
            Action::Rejected(Some(1))
        ));
        assert!(matches!(
            slot.transition(Event::Attach {
                ticket: b,
                child: 2
            }),
            Action::Accepted
        ));
        assert!(slot.is_none());
        assert!(matches!(
            slot.transition(Event::Ready(a)),
            Action::Rejected(None)
        ));
        assert!(matches!(slot.transition(Event::Ready(b)), Action::Accepted));
        assert_eq!(slot.current(), Some(2));
    }
    #[test]
    fn shutdown_takes_the_unpublished_child_and_absorbs_old_events() {
        let mut slot = PiSlot::default();
        let a = begin(&mut slot);
        slot.transition(Event::Attach {
            ticket: a,
            child: 1,
        });
        assert!(matches!(
            slot.transition(Event::Shutdown),
            Action::Retired(Some(1))
        ));
        for event in [
            Event::Ready(a),
            Event::Abort(a),
            Event::Begin,
            Event::Park(a),
            Event::Retire(a),
            Event::Exited(a),
        ] {
            assert!(matches!(slot.transition(event), Action::Rejected(None)));
            assert!(slot.is_stopped());
        }
        assert!(matches!(
            slot.transition(Event::Attach {
                ticket: a,
                child: 2
            }),
            Action::Rejected(Some(2))
        ));
    }
    #[test]
    fn abort_only_releases_its_unpublished_owner() {
        let mut slot = PiSlot::default();
        let a = begin(&mut slot);
        let b = begin(&mut slot);
        slot.transition(Event::Attach {
            ticket: b,
            child: 2,
        });
        assert!(matches!(
            slot.transition(Event::Abort(a)),
            Action::Rejected(None)
        ));
        assert!(matches!(
            slot.transition(Event::Abort(b)),
            Action::Retired(Some(2))
        ));
        assert!(slot.is_none());
    }
    #[test]
    fn exited_handle_survives_until_retirement_and_park_can_wake() {
        let mut slot = PiSlot::default();
        let a = begin(&mut slot);
        slot.transition(Event::Attach {
            ticket: a,
            child: 1,
        });
        slot.transition(Event::Ready(a));
        assert!(slot.is_ready());
        assert!(slot.is_some());
        assert!(!slot.is_booting());
        slot.transition(Event::Exited(a));
        assert!(!slot.is_ready());
        assert_eq!(slot.current(), Some(1));
        assert!(matches!(
            slot.transition(Event::Park(a)),
            Action::Retired(Some(1))
        ));
        assert!(slot.is_parked());
        let b = begin(&mut slot);
        assert_ne!(a, b);
        assert!(slot.is_booting());
    }
}
