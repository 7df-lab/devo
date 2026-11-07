/// Host-owned agent identity for prompt assembly. This is derived from session
/// ancestry at each turn, not inherited from a parent's locked prompt snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum AgentPromptIdentity {
    #[default]
    Root,
    Child {
        depth: u32,
        parent: String,
    },
}
