use devo_protocol::{PendingInputItem, TurnKind};

#[derive(Debug)]
pub(crate) struct TurnState {
    pub pending_input: Vec<PendingInputItem>,
}

impl TurnState {
    pub fn new(_kind: TurnKind) -> Self {
        Self {
            pending_input: Vec::new(),
        }
    }

    pub fn push_pending_input(&mut self, item: PendingInputItem) {
        self.pending_input.push(item);
    }

    pub fn take_pending_input(&mut self) -> Vec<PendingInputItem> {
        std::mem::take(&mut self.pending_input)
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use pretty_assertions::assert_eq;

    use super::*;

    fn sample_item(text: &str) -> PendingInputItem {
        PendingInputItem::new(
            devo_protocol::PendingInputKind::UserText { text: text.into() },
            None,
            Utc::now(),
        )
    }

    #[test]
    fn turn_state_new_has_empty_pending_input() {
        let mut state = TurnState::new(TurnKind::Regular);
        assert!(state.take_pending_input().is_empty());
    }

    #[test]
    fn turn_state_push_and_take() {
        let mut state = TurnState::new(TurnKind::Regular);
        state.push_pending_input(sample_item("test"));
        let taken = state.take_pending_input();
        assert_eq!(taken.len(), 1);
        assert!(state.take_pending_input().is_empty());
    }

    #[test]
    fn turn_state_multiple_pushes() {
        let mut state = TurnState::new(TurnKind::Regular);
        state.push_pending_input(sample_item("a"));
        state.push_pending_input(sample_item("b"));
        state.push_pending_input(sample_item("c"));
        assert_eq!(state.take_pending_input().len(), 3);
    }
}
