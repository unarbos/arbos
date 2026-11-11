//! Local UI/coordinator admission. Tickets never cross the relay wire.
use super::work_lifetime::{WorkLifetime, WorkTicket};
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Clone)]
pub struct UiControl(Arc<Mutex<WorkLifetime>>);

impl std::fmt::Debug for UiControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UiControl")
    }
}

impl Default for UiControl {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(WorkLifetime::default())))
    }
}

pub(crate) struct UiAdmission<'a> {
    _owner: MutexGuard<'a, WorkLifetime>,
}

impl UiControl {
    pub(crate) fn ticket(&self) -> Option<WorkTicket> {
        self.0.lock().ok()?.ticket()
    }
    pub(crate) fn accepts(&self, ticket: WorkTicket) -> bool {
        self.0.lock().is_ok_and(|owner| owner.accepts(ticket))
    }
    /// Fence a short credential/publication commit; never await or destroy SDK resources here.
    pub(crate) fn admit(&self, ticket: WorkTicket) -> Option<UiAdmission<'_>> {
        let owner = self.0.lock().ok()?;
        if !owner.accepts(ticket) {
            return None;
        }
        Some(UiAdmission { _owner: owner })
    }
    fn renew(&self) -> Option<WorkTicket> {
        Some(self.0.lock().ok()?.reopen())
    }
}

#[derive(Clone, Copy)]
enum Binding {
    Waiting,
    Active(WorkTicket),
}

pub(crate) struct UiSession {
    pub(crate) control: UiControl,
    binding: Binding,
}

impl Default for UiSession {
    fn default() -> Self {
        Self {
            control: UiControl::default(),
            binding: Binding::Waiting,
        }
    }
}

impl UiSession {
    pub(crate) fn account(&self) -> Option<WorkTicket> {
        match self.binding {
            Binding::Waiting => None,
            Binding::Active(owner) => Some(owner),
        }
    }
    pub(crate) fn accepts_account(&self, owner: WorkTicket) -> bool {
        self.account() == Some(owner)
    }
    pub(crate) fn accepts_account_work(
        &self,
        control: Option<WorkTicket>,
        account: Option<WorkTicket>,
    ) -> bool {
        match (control, account) {
            (Some(control), Some(account)) => {
                self.control.accepts(control) && self.accepts_account(account)
            }
            _ => false,
        }
    }
    pub(crate) fn begin_control(&self) -> Option<WorkTicket> {
        self.control.renew()
    }
    pub(crate) fn invalidate(&mut self) -> Option<WorkTicket> {
        self.binding = Binding::Waiting;
        self.control.renew()
    }
    pub(crate) fn adopt(&mut self, control: WorkTicket, account: WorkTicket) -> bool {
        if !self.control.accepts(control) {
            return false;
        }
        self.binding = Binding::Active(account);
        true
    }
    pub(crate) fn clear_account(&mut self) {
        self.binding = Binding::Waiting;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_signout_rejects_an_already_queued_session_start() {
        let mut ui = UiSession::default();
        let coordinator = ui.control.clone();
        let old_control = coordinator.ticket().unwrap();
        let account = WorkLifetime::default().ticket().unwrap();
        ui.invalidate().unwrap();
        assert!(!coordinator.accepts(old_control));
        assert!(!ui.adopt(old_control, account));
        assert!(ui.account().is_none());
    }
    #[test]
    fn replacement_rejects_queued_old_data_and_keeps_current_control() {
        let mut ui = UiSession::default();
        let old = WorkLifetime::default().ticket().unwrap();
        let next = WorkLifetime::default().ticket().unwrap();
        let old_control = ui.control.ticket().unwrap();
        assert!(ui.adopt(old_control, old));
        let next_control = ui.invalidate().unwrap();
        assert!(ui.adopt(next_control, next));
        assert!(!ui.accepts_account(old));
        assert!(ui.accepts_account(next));
    }
    #[test]
    fn backend_replacement_under_same_control_refuses_old_account_work() {
        let mut ui = UiSession::default();
        let control = ui.control.ticket().unwrap();
        let old = WorkLifetime::default().ticket().unwrap();
        let next = WorkLifetime::default().ticket().unwrap();
        assert!(ui.adopt(control, old));
        assert!(ui.accepts_account_work(Some(control), Some(old)));
        assert!(ui.adopt(control, next));
        assert!(ui.control.accepts(control));
        assert!(!ui.accepts_account_work(Some(control), Some(old)));
        assert!(ui.accepts_account_work(Some(control), Some(next)));
        assert!(!ui.accepts_account_work(Some(control), None));
    }
    #[test]
    fn old_async_intent_cannot_take_the_new_control_ticket() {
        let ui = UiSession::default();
        let captured = ui.control.ticket().unwrap();
        let current = ui.begin_control().unwrap();
        assert!(!ui.control.accepts(captured));
        assert!(ui.control.accepts(current));
    }
}
