//! Per-session RPC resource admission. Effects run after releasing the registry lock.
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Ticket(u64);

pub(crate) enum Entry<T> {
    Starting { ticket: Ticket, resource: Option<T> },
    Ready { ticket: Ticket, resource: T },
    Stopping { ticket: Ticket, resource: Option<T> },
}
impl<T> Entry<T> {
    pub(crate) fn ready(&self) -> Option<&T> {
        match self {
            Self::Ready { resource, .. } => Some(resource),
            _ => None,
        }
    }
    pub(crate) fn starting(&self, ticket: Ticket) -> bool {
        matches!(self, Self::Starting { ticket: current, resource: Some(_) } if *current == ticket)
    }
    pub(crate) fn stopping(&self) -> bool {
        matches!(self, Self::Stopping { .. })
    }
    fn into_resource(self) -> Option<T> {
        match self {
            Self::Starting { resource, .. } | Self::Stopping { resource, .. } => resource,
            Self::Ready { resource, .. } => Some(resource),
        }
    }
}

pub(crate) enum State<T> {
    Accepting {
        next: u64,
        entries: HashMap<String, Entry<T>>,
    },
    Closed,
}
impl<T> Default for State<T> {
    fn default() -> Self {
        Self::Accepting {
            next: 0,
            entries: HashMap::new(),
        }
    }
}

pub(crate) enum Event<T> {
    Begin(String),
    Capture {
        id: String,
        ticket: Ticket,
        resource: T,
    },
    Publish {
        id: String,
        ticket: Ticket,
    },
    Abort {
        id: String,
        ticket: Ticket,
    },
    Stop(String),
    Stopped {
        id: String,
        ticket: Ticket,
    },
    Shutdown,
}
pub(crate) enum Action<T> {
    Begun { ticket: Ticket, retired: Option<T> },
    Stopping { ticket: Ticket, resource: Option<T> },
    Retired(Option<T>),
    Accepted,
    Missing,
    Wait,
    Closed(Vec<T>),
    Rejected(Option<T>),
}
impl<T: Clone> State<T> {
    pub(crate) fn entry(&self, id: &str) -> Option<&Entry<T>> {
        match self {
            Self::Accepting { entries, .. } => entries.get(id),
            Self::Closed => None,
        }
    }
    pub(crate) fn ready(&self) -> impl Iterator<Item = (&str, &T)> {
        let entries = match self {
            Self::Accepting { entries, .. } => Some(entries),
            Self::Closed => None,
        };
        entries
            .into_iter()
            .flat_map(|entries| entries.iter())
            .filter_map(|(id, entry)| entry.ready().map(|resource| (id.as_str(), resource)))
    }
    /// All resource publication and retirement decisions use the same exact-ticket boundary.
    pub(crate) fn transition(&mut self, event: Event<T>) -> Action<T> {
        if matches!(event, Event::Shutdown) {
            let old = std::mem::replace(self, Self::Closed);
            return Action::Closed(match old {
                Self::Accepting { entries, .. } => entries
                    .into_values()
                    .filter_map(Entry::into_resource)
                    .collect(),
                Self::Closed => Vec::new(),
            });
        }
        let Self::Accepting { next, entries } = self else {
            return match event {
                Event::Capture { resource, .. } => Action::Rejected(Some(resource)),
                _ => Action::Closed(Vec::new()),
            };
        };
        match event {
            Event::Begin(id) => {
                if entries.get(&id).is_some_and(Entry::stopping) {
                    return Action::Wait;
                }
                let Some(value) = next.checked_add(1) else {
                    let resources = entries
                        .drain()
                        .filter_map(|(_, entry)| entry.into_resource())
                        .collect();
                    *self = Self::Closed;
                    return Action::Closed(resources);
                };
                *next = value;
                let ticket = Ticket(value);
                let retired = entries
                    .insert(
                        id,
                        Entry::Starting {
                            ticket,
                            resource: None,
                        },
                    )
                    .and_then(Entry::into_resource);
                Action::Begun { ticket, retired }
            }
            Event::Capture {
                id,
                ticket,
                resource,
            } => match entries.get_mut(&id) {
                Some(Entry::Starting {
                    ticket: current,
                    resource: held,
                }) if *current == ticket && held.is_none() => {
                    *held = Some(resource);
                    Action::Accepted
                }
                _ => Action::Rejected(Some(resource)),
            },
            Event::Publish { id, ticket } => {
                if !matches!(entries.get(&id), Some(Entry::Starting { ticket: current, resource: Some(_) }) if *current == ticket)
                {
                    return Action::Rejected(None);
                }
                let Some(Entry::Starting {
                    resource: Some(resource),
                    ..
                }) = entries.remove(&id)
                else {
                    unreachable!()
                };
                entries.insert(id, Entry::Ready { ticket, resource });
                Action::Accepted
            }
            Event::Abort { id, ticket } => {
                if matches!(entries.get(&id), Some(Entry::Starting { ticket: current, .. }) if *current == ticket)
                {
                    Action::Retired(entries.remove(&id).and_then(Entry::into_resource))
                } else {
                    Action::Rejected(None)
                }
            }
            Event::Stop(id) => {
                if entries.get(&id).is_some_and(Entry::stopping) {
                    return Action::Wait;
                }
                let Some(entry) = entries.remove(&id) else {
                    return Action::Missing;
                };
                let ticket = match &entry {
                    Entry::Starting { ticket, .. } | Entry::Ready { ticket, .. } => *ticket,
                    Entry::Stopping { .. } => unreachable!(),
                };
                let resource = entry.into_resource();
                entries.insert(
                    id,
                    Entry::Stopping {
                        ticket,
                        resource: resource.clone(),
                    },
                );
                Action::Stopping { ticket, resource }
            }
            Event::Stopped { id, ticket } => {
                if matches!(entries.get(&id), Some(Entry::Stopping { ticket: current, .. }) if *current == ticket)
                {
                    entries.remove(&id);
                    Action::Accepted
                } else {
                    Action::Rejected(None)
                }
            }
            Event::Shutdown => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn begin(state: &mut State<u32>) -> Ticket {
        match state.transition(Event::Begin("s".into())) {
            Action::Begun { ticket, .. } => ticket,
            _ => panic!("begin refused"),
        }
    }
    #[test]
    fn shutdown_refuses_late_capture_and_publication() {
        let mut state = State::default();
        let ticket = begin(&mut state);
        state.transition(Event::Shutdown);
        assert!(matches!(
            state.transition(Event::Capture {
                id: "s".into(),
                ticket,
                resource: 7
            }),
            Action::Rejected(Some(7))
        ));
        assert!(matches!(
            state.transition(Event::Publish {
                id: "s".into(),
                ticket
            }),
            Action::Closed(_)
        ));
        assert!(matches!(
            state.transition(Event::Begin("s".into())),
            Action::Closed(_)
        ));
    }
    #[test]
    fn stale_abort_and_stop_completion_preserve_successor() {
        let mut state = State::default();
        let old = begin(&mut state);
        let current = begin(&mut state);
        assert!(matches!(
            state.transition(Event::Abort {
                id: "s".into(),
                ticket: old
            }),
            Action::Rejected(_)
        ));
        state.transition(Event::Capture {
            id: "s".into(),
            ticket: current,
            resource: 9,
        });
        state.transition(Event::Publish {
            id: "s".into(),
            ticket: current,
        });
        state.transition(Event::Stop("s".into()));
        assert!(matches!(
            state.transition(Event::Stopped {
                id: "s".into(),
                ticket: old
            }),
            Action::Rejected(_)
        ));
        assert!(state.entry("s").unwrap().stopping());
        state.transition(Event::Stopped {
            id: "s".into(),
            ticket: current,
        });
        assert!(state.entry("s").is_none());
        assert!(matches!(
            state.transition(Event::Begin("s".into())),
            Action::Begun { .. }
        ));
    }
    #[test]
    fn stopping_waits_and_shutdown_owns_its_resource() {
        let mut state = State::default();
        let ticket = begin(&mut state);
        state.transition(Event::Capture {
            id: "s".into(),
            ticket,
            resource: 7,
        });
        assert!(matches!(
            state.transition(Event::Stop("s".into())),
            Action::Stopping {
                resource: Some(7),
                ..
            }
        ));
        assert!(matches!(
            state.transition(Event::Begin("s".into())),
            Action::Wait
        ));
        assert!(
            matches!(state.transition(Event::Shutdown),Action::Closed(resources) if resources == vec![7])
        );
    }
}
