use crate::error::{Error, Result};

pub const NONE32: u32 = 0xFFFF_FFFF;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Kind {
    Applicability = 1,
    Prose = 2,
    Contract = 3,
    Segment = 4,
    Resource = 5,
    Binding = 6,
}

impl Kind {
    pub(crate) fn from_u8(v: u8) -> Result<Self> {
        Ok(match v {
            1 => Self::Applicability,
            2 => Self::Prose,
            3 => Self::Contract,
            4 => Self::Segment,
            5 => Self::Resource,
            6 => Self::Binding,
            other => return Err(Error::UnknownNodeKind(other)),
        })
    }
}

/// `CONTAINS` is deliberately absent: the tree is carried by `Node::parent` plus pre-order
/// position, and storing it again as an edge would be a second encoding of one fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum EdgeKind {
    Seq = 2,
    Guards = 3,
    Needs = 4,
    Alt = 5,
    Cites = 6,
}

pub(crate) const EDGE_EXTERNAL: u8 = 0x80;
pub(crate) const CONTAINS_TAG: u8 = 1;

impl EdgeKind {
    pub(crate) fn from_u8(v: u8) -> Result<Self> {
        Ok(match v {
            2 => Self::Seq,
            3 => Self::Guards,
            4 => Self::Needs,
            5 => Self::Alt,
            6 => Self::Cites,
            other => return Err(Error::UnknownEdgeKind(other)),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Tier {
    Routing = 0,
    Body = 1,
    OnDemand = 2,
}

impl Tier {
    pub(crate) fn from_u8(v: u8) -> Result<Self> {
        Ok(match v {
            0 => Self::Routing,
            1 => Self::Body,
            2 => Self::OnDemand,
            other => return Err(Error::UnknownTier(other)),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum Abi {
    Wasm32Wasip2 = 1,
    Sh = 2,
    Python3 = 3,
    Node = 4,
    Native = 5,
}

impl Abi {
    pub(crate) fn from_u16(v: u16) -> Result<Self> {
        Ok(match v {
            1 => Self::Wasm32Wasip2,
            2 => Self::Sh,
            3 => Self::Python3,
            4 => Self::Node,
            5 => Self::Native,
            other => return Err(Error::UnknownAbi(other)),
        })
    }

    #[must_use]
    pub fn is_portable(self) -> bool {
        matches!(self, Self::Wasm32Wasip2)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TrustClass {
    Portable = 0,
    HostTrusted = 1,
}

pub mod node_flags {
    pub const MUST_UNDERSTAND: u8 = 1 << 0;
    pub const PAYLOAD_COLD: u8 = 1 << 1;
    pub const PAYLOAD_COMPRESSED: u8 = 1 << 2;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Node {
    pub kind: Kind,
    pub tier: Tier,
    pub flags: u8,
    pub depth: u8,
    pub role: u16,
    pub edge_cnt: u16,
    pub name_idx: u32,
    pub edge_off: u32,
    pub payload_off: u32,
    pub payload_len: u32,
    pub hash_idx: u32,
    pub parent: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    pub dst: u32,
    pub kind: EdgeKind,
    pub external: bool,
    pub ordinal: u8,
    pub label: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub root: [u8; 32],
    pub off: u32,
    pub len: u32,
    pub orig_len: u32,
    pub abi: Abi,
    pub codec: u8,
    pub trust_class: TrustClass,
    pub cap_off: u32,
    pub cap_cnt: u16,
    pub flags: u16,
    pub mem_kib: u32,
    pub cpu_ms: u32,
    pub wall_ms: u32,
    pub dict_idx: u32,
    pub signer_idx: u32,
}
