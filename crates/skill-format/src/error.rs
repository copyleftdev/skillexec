use core::fmt;

/// Every rejection names the step of `SPEC.md` §5 that produced it, so the hostile
/// corpus can assert *where* a malformed file dies, not merely that it does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    BadMagic,
    UnsupportedMajor(u16),
    UnknownFeatureFlags(u32),
    LengthMismatch {
        declared: u32,
        actual: usize,
    },
    Truncated {
        off: usize,
        need: usize,
    },
    OffsetOverflow {
        what: &'static str,
    },
    RangeOutOfBounds {
        what: &'static str,
        off: u32,
        len: u32,
    },
    RangeOverlap {
        a: u32,
        b: u32,
    },
    ReservedNonZero {
        what: &'static str,
    },
    Unsigned,
    UntrustedKey(Box<[u8; 32]>),
    BadSignature(u16),
    DuplicateKeyId,
    ManifestDigestMismatch,
    UnknownSection(u16),
    DuplicateSection(u16),
    SectionMisaligned(u16),
    SectionLenNotMultiple {
        id: u16,
        len: u32,
        unit: usize,
    },
    SectionCountMismatch {
        id: u16,
    },
    UnknownNodeKind(u8),
    UnknownEdgeKind(u8),
    UnknownTier(u8),
    UnknownAbi(u16),
    NodeIndexOutOfRange(u32),
    StringIndexOutOfRange(u32),
    HashIndexOutOfRange(u32),
    RootHasParent,
    RootNotRouting,
    UncommittedNonZero {
        region: &'static str,
        off: u32,
    },
    NonRootWithoutParent(u32),
    ParentNotBefore {
        node: u32,
        parent: u32,
    },
    NotPreOrder(u32),
    ContainsInEdgeTable(u32),
    SeqNotForward {
        src: u32,
        dst: u32,
    },
    NeedsCycle(u32),
    ObligationCycle(u32),
    NeedsBadTarget {
        src: u32,
        dst: u32,
    },
    TierNotMonotone {
        node: u32,
        parent: u32,
    },
    GuardsFromNonContract(u32),
    DuplicateAltOrdinal {
        src: u32,
        ordinal: u8,
    },
    StringHeapUnsorted(u32),
    PayloadHashMismatch(u32),
    SegmentHashMismatch(u32),
    SegmentIndexOutOfRange(u32),
    PortableClaimOnNonWasm(u32),
    SegmentCountMismatch {
        nodes: u32,
        records: u32,
    },
    DeclaredLenMismatch {
        what: &'static str,
    },
    RootHasNoCommitment,
    RoutingNotUtf8,
    RoutingEmpty,
    RoutingRootNotTopLevel(u32),
    RoutingTooLong,
    RoutingUncommitted,
    RoutingHashMismatch,
    MissingDictionary,
    WrongDictionary,
    UncommittedRegion(&'static str),
    RegionHashMismatch(&'static str),
}

impl Error {
    /// The `SPEC.md` §5 verification step at which a conforming reader rejects this file.
    #[must_use]
    pub fn step(&self) -> u8 {
        match self {
            Self::BadMagic | Self::UnsupportedMajor(_) | Self::UnknownFeatureFlags(_) => 1,
            Self::LengthMismatch { .. } => 2,
            Self::Unsigned
            | Self::UntrustedKey(_)
            | Self::BadSignature(_)
            | Self::DuplicateKeyId => 3,
            Self::Truncated { .. }
            | Self::OffsetOverflow { .. }
            | Self::RangeOutOfBounds { .. } => 4,
            Self::ManifestDigestMismatch => 5,
            Self::PayloadHashMismatch(_)
            | Self::SegmentHashMismatch(_)
            | Self::RegionHashMismatch(_) => 7,
            _ => 6,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?} (rejected at SPEC §5 step {})", self.step())
    }
}

impl std::error::Error for Error {}

pub type Result<T> = core::result::Result<T, Error>;
