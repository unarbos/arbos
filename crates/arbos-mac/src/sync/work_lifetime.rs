//! Local tickets for account work and Matrix callbacks. No token or remote identity is serialized.
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);
fn owner_id() -> u64 {
    let mut current = NEXT_OWNER.load(Ordering::Relaxed);
    loop {
        let next = current.checked_add(1).expect("work owner exhausted");
        match NEXT_OWNER.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
        {
            Ok(_) => return current,
            Err(observed) => current = observed,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WorkTicket {
    owner: u64,
    generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkState {
    Accepting(u64),
    Closed(u64),
}

#[derive(Clone, Copy)]
pub(crate) enum WorkEvent {
    Reopen,
    Close,
}

pub(crate) struct WorkLifetime {
    owner: u64,
    state: WorkState,
}

impl Default for WorkLifetime {
    fn default() -> Self {
        Self {
            owner: owner_id(),
            state: WorkState::Accepting(0),
        }
    }
}

impl WorkLifetime {
    pub(crate) fn transition(state: WorkState, event: WorkEvent) -> WorkState {
        let generation = match state {
            WorkState::Accepting(n) | WorkState::Closed(n) => n,
        }
        .checked_add(1)
        .expect("work lifetime exhausted");
        match event {
            WorkEvent::Reopen => WorkState::Accepting(generation),
            WorkEvent::Close => WorkState::Closed(generation),
        }
    }

    pub(crate) fn ticket(&self) -> Option<WorkTicket> {
        match self.state {
            WorkState::Accepting(generation) => Some(WorkTicket {
                owner: self.owner,
                generation,
            }),
            WorkState::Closed(_) => None,
        }
    }

    pub(crate) fn accepts(&self, ticket: WorkTicket) -> bool {
        self.ticket() == Some(ticket)
    }
    pub(crate) fn reopen(&mut self) -> WorkTicket {
        self.state = Self::transition(self.state, WorkEvent::Reopen);
        self.ticket().expect("reopened lifetime")
    }
    pub(crate) fn close(&mut self) {
        self.state = Self::transition(self.state, WorkEvent::Close);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MatrixTicket {
    pub(crate) account: WorkTicket,
    attempt: WorkTicket,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MatrixState {
    Idle,
    Starting(MatrixTicket),
    Running(MatrixTicket),
}

#[derive(Clone, Copy)]
enum MatrixEvent {
    Start(MatrixTicket),
    Ready(MatrixTicket),
    Failed(MatrixTicket),
    Stop,
}

pub(crate) struct MatrixLifetime {
    attempts: WorkLifetime,
    state: MatrixState,
}

impl Default for MatrixLifetime {
    fn default() -> Self {
        Self {
            attempts: WorkLifetime::default(),
            state: MatrixState::Idle,
        }
    }
}

impl MatrixLifetime {
    fn transition(state: MatrixState, event: MatrixEvent) -> MatrixState {
        match (state, event) {
            (MatrixState::Idle, MatrixEvent::Start(ticket)) => MatrixState::Starting(ticket),
            (MatrixState::Starting(current), MatrixEvent::Ready(ticket)) if current == ticket => {
                MatrixState::Running(ticket)
            }
            (MatrixState::Starting(current), MatrixEvent::Failed(ticket)) if current == ticket => {
                MatrixState::Idle
            }
            (_, MatrixEvent::Stop) => MatrixState::Idle,
            _ => state,
        }
    }
    pub(crate) fn start(&mut self, account: WorkTicket) -> Option<MatrixTicket> {
        if self.state != MatrixState::Idle {
            return None;
        }
        let ticket = MatrixTicket {
            account,
            attempt: self.attempts.reopen(),
        };
        self.state = Self::transition(self.state, MatrixEvent::Start(ticket));
        Some(ticket)
    }
    pub(crate) fn accepts(&self, ticket: MatrixTicket) -> bool {
        self.attempts.accepts(ticket.attempt)
            && match self.state {
                MatrixState::Starting(current) | MatrixState::Running(current) => current == ticket,
                MatrixState::Idle => false,
            }
    }
    pub(crate) fn ready(&mut self, ticket: MatrixTicket) {
        self.state = Self::transition(self.state, MatrixEvent::Ready(ticket));
    }
    pub(crate) fn failed(&mut self, ticket: MatrixTicket) {
        self.state = Self::transition(self.state, MatrixEvent::Failed(ticket));
    }
    pub(crate) fn stop(&mut self) {
        self.attempts.close();
        self.state = Self::transition(self.state, MatrixEvent::Stop);
    }
}
