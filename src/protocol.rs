/// Description of a single network protocol.
pub trait Protocol {
    /// Return the lowercase name of the protocol. A version number must be included if there are
    /// multiple incompatible versions (e.g. SSH1 vs SSH2).
    fn name(&self) -> &str;
}
