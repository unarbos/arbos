//! Pure ownership of a planned restart; socket/work signals remain effects at the edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Freezing,
    Notifying,
    Draining,
    Ready,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    Serving(u64),
    Planned { generation: u64, phase: Phase },
    Shutdown,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Event {
    Plan,
    Advance { generation: u64, phase: Phase },
    Release(u64),
    Shutdown,
}

pub(crate) fn transition(state: State, event: Event) -> State {
    match (state, event) {
        (State::Shutdown, _) | (_, Event::Shutdown) => State::Shutdown,
        (State::Serving(previous), Event::Plan) => State::Planned {
            generation: previous
                .checked_add(1)
                .expect("restart generation exhausted"),
            phase: Phase::Freezing,
        },
        (
            State::Planned {
                generation,
                phase: previous,
            },
            Event::Advance {
                generation: owner,
                phase,
            },
        ) if generation == owner
            && matches!(
                (previous, phase),
                (Phase::Freezing, Phase::Notifying)
                    | (Phase::Notifying, Phase::Draining)
                    | (Phase::Draining, Phase::Ready)
            ) =>
        {
            State::Planned { generation, phase }
        }
        (State::Planned { generation, .. }, Event::Release(owner)) if generation == owner => {
            State::Serving(generation)
        }
        _ => state,
    }
}
