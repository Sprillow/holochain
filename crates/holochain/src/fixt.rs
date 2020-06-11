pub mod curve;

use crate::core::ribosome::wasm_ribosome::WasmRibosome;
use crate::core::ribosome::FnComponents;
use crate::core::ribosome::HostContextFixturator;
use crate::core::state::metadata::LinkMetaVal;
use fixt::prelude::*;
use holo_hash::AgentPubKeyFixturator;
use holo_hash::DnaHashFixturator;
use holo_hash::EntryContentHashFixturator;
use holo_hash::HeaderHashFixturator;
use holo_hash::HoloHashExt;
use holo_hash::WasmHash;
use holo_hash_core::HeaderHash;
use holochain_types::composite_hash::EntryHash;
use holochain_types::dna::wasm::DnaWasm;
use holochain_types::dna::zome::Zome;
use holochain_types::dna::DnaDef;
use holochain_types::dna::DnaFile;
use holochain_types::dna::Wasms;
use holochain_types::dna::Zomes;
use holochain_types::fixt::HeaderBuilderCommonFixturator;
use holochain_types::fixt::TimestampFixturator;
use holochain_types::header::builder::HeaderBuilder;
use holochain_types::header::builder::LinkAdd as LinkAddBuilder;
use holochain_types::header::builder::LinkRemove as LinkRemoveBuilder;
use holochain_types::header::LinkAdd;
use holochain_types::header::{HeaderBuilderCommon, LinkRemove, ZomeId};
use holochain_types::link::Tag;
use holochain_types::test_utils::fake_dna_zomes;
use holochain_wasm_test_utils::strum::IntoEnumIterator;
use holochain_wasm_test_utils::TestWasm;
use holochain_zome_types::capability::CapAccess;
use holochain_zome_types::capability::CapClaim;
use holochain_zome_types::capability::CapGrant;
use holochain_zome_types::capability::CapSecret;
use holochain_zome_types::capability::GrantedFunctions;
use holochain_zome_types::capability::ZomeCallCapGrant;
use holochain_zome_types::header::HeaderHashes;
use holochain_zome_types::migrate_agent::MigrateAgent;
use holochain_zome_types::zome::ZomeName;
use holochain_zome_types::Entry;
use holochain_zome_types::HostInput;
use rand::seq::IteratorRandom;
use rand::thread_rng;
use rand::Rng;
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::sync::Arc;

wasm_io_fixturator!(HostInput<SerializedBytes>);

newtype_fixturator!(ZomeName<String>);

newtype_fixturator!(FnComponents<Vec<String>>);

fixturator!(
    MigrateAgent;
    unit variants [ Open Close ] empty Close;
);

fixturator!(
    ZomeCallCapGrant,
    {
        ZomeCallCapGrant::new(
            StringFixturator::new(Empty).next().unwrap(),
            CapAccessFixturator::new(Empty).next().unwrap(),
            {
                let mut rng = rand::thread_rng();
                let number_of_zomes = rng.gen_range(0, 5);

                let mut granted_functions: GrantedFunctions = BTreeMap::new();
                for _ in 0..number_of_zomes {
                    let number_of_functions = rng.gen_range(0, 5);
                    let mut zome_functions = vec![];
                    for _ in 0..number_of_functions {
                        zome_functions.push(StringFixturator::new(Empty).next().unwrap());
                    }
                    granted_functions.insert(
                        ZomeNameFixturator::new(Empty).next().unwrap(),
                        zome_functions,
                    );
                }
                granted_functions
            },
        )
    },
    {
        ZomeCallCapGrant::new(
            StringFixturator::new(Unpredictable).next().unwrap(),
            CapAccessFixturator::new(Unpredictable).next().unwrap(),
            {
                let mut rng = rand::thread_rng();
                let number_of_zomes = rng.gen_range(0, 5);

                let mut granted_functions: GrantedFunctions = BTreeMap::new();
                for _ in 0..number_of_zomes {
                    let number_of_functions = rng.gen_range(0, 5);
                    let mut zome_functions = vec![];
                    for _ in 0..number_of_functions {
                        zome_functions.push(StringFixturator::new(Unpredictable).next().unwrap());
                    }
                    granted_functions.insert(
                        ZomeNameFixturator::new(Unpredictable).next().unwrap(),
                        zome_functions,
                    );
                }
                granted_functions
            },
        )
    },
    {
        ZomeCallCapGrant::new(
            StringFixturator::new_indexed(Predictable, self.0.index)
                .next()
                .unwrap(),
            CapAccessFixturator::new_indexed(Predictable, self.0.index)
                .next()
                .unwrap(),
            {
                let mut granted_functions: GrantedFunctions = BTreeMap::new();
                for _ in 0..self.0.index % 3 {
                    let number_of_functions = self.0.index % 3;
                    let mut zome_functions = vec![];
                    for _ in 0..number_of_functions {
                        zome_functions.push(StringFixturator::new(Predictable).next().unwrap());
                    }
                    granted_functions.insert(
                        ZomeNameFixturator::new(Predictable).next().unwrap(),
                        zome_functions,
                    );
                }
                granted_functions
            },
        )
    }
);

fixturator!(
    CapSecret;
    from String;
);

fixturator!(
    CapAccess;

    enum [ Unrestricted Transferable Assigned ];

    curve Empty {
        match CapAccessVariant::random() {
            Unrestricted => CapAccess::unrestricted(),
            Transferable => CapAccess::transferable(),
            Assigned => CapAccess::assigned({
                let mut set = HashSet::new();
                set.insert(fixt!(AgentPubKey, Empty).into());
                set
            })
        }
    };

    curve Unpredictable {
        match CapAccessVariant::random() {
            Unrestricted => CapAccess::unrestricted(),
            Transferable => CapAccess::transferable(),
            Assigned => CapAccess::assigned({
                let mut set = HashSet::new();
                set.insert(fixt!(AgentPubKey).into());
                set
            })
        }
    };

    curve Predictable {
        match CapAccessVariant::nth(self.0.index) {
            Unrestricted => CapAccess::unrestricted(),
            Transferable => CapAccess::transferable(),
            Assigned => CapAccess::assigned({
                let mut set = HashSet::new();
                set.insert(AgentPubKeyFixturator::new_indexed(Predictable, self.0.index).next().unwrap().into());
                set
            })
        }
    };
);

fixturator!(
    CapGrant;
    variants [ Authorship(AgentPubKey) ZomeCall(ZomeCallCapGrant) ];
);

fixturator!(
    CapClaim;
    constructor fn new(String, AgentPubKey, CapSecret);
);

fixturator!(
    Entry;
    variants [
        Agent(AgentPubKey)
        App(SerializedBytes)
        CapClaim(CapClaim)
        CapGrant(ZomeCallCapGrant)
    ];
);

fixturator!(
    HeaderHashes,
    vec![].into(),
    {
        let mut rng = rand::thread_rng();
        let number_of_hashes = rng.gen_range(0, 5);

        let mut hashes: Vec<HeaderHash> = vec![];
        let mut header_hash_fixturator = HeaderHashFixturator::new(Unpredictable);
        for _ in (0..number_of_hashes) {
            hashes.push(header_hash_fixturator.next().unwrap().into());
        }
        hashes.into()
    },
    {
        let mut hashes: Vec<HeaderHash> = vec![];
        let mut header_hash_fixturator =
            HeaderHashFixturator::new_indexed(Predictable, self.0.index);
        for _ in 0..3 {
            hashes.push(header_hash_fixturator.next().unwrap().into());
        }
        hashes.into()
    }
);

fixturator!(
    Wasms,
    { BTreeMap::new() },
    {
        let mut rng = rand::thread_rng();
        let number_of_wasms = rng.gen_range(0, 5);

        let mut wasms: Wasms = BTreeMap::new();
        let mut dna_wasm_fixturator = DnaWasmFixturator::new(Unpredictable);
        for _ in (0..number_of_wasms) {
            let wasm = dna_wasm_fixturator.next().unwrap();
            wasms.insert(
                tokio_safe_block_on::tokio_safe_block_on(
                    async { WasmHash::with_data(wasm.code().to_vec()).await },
                    std::time::Duration::from_millis(10),
                )
                .unwrap()
                .into(),
                wasm,
            );
        }
        wasms
    },
    {
        let mut wasms: Wasms = BTreeMap::new();
        let mut dna_wasm_fixturator = DnaWasmFixturator::new_indexed(Predictable, self.0.index);
        for _ in (0..3) {
            let wasm = dna_wasm_fixturator.next().unwrap();
            wasms.insert(
                tokio_safe_block_on::tokio_safe_block_on(
                    async { WasmHash::with_data(wasm.code().to_vec()).await },
                    std::time::Duration::from_millis(10),
                )
                .unwrap()
                .into(),
                wasm,
            );
        }
        wasms
    }
);

fixturator!(
    Zomes,
    Vec::new(),
    {
        // @todo implement unpredictable zomes
        ZomesFixturator::new(Empty).next().unwrap()
    },
    {
        // @todo implement predictable zomes
        ZomesFixturator::new(Empty).next().unwrap()
    }
);

fixturator!(
    DnaWasm;
    // note that an empty wasm will not compile
    curve Empty DnaWasm { code: Arc::new(vec![]) };
    curve Unpredictable TestWasm::iter().choose(&mut thread_rng()).unwrap().into();
    curve Predictable TestWasm::iter().cycle().nth(self.0.index).unwrap().into();
);

fixturator!(
    DnaDef;
    curve Empty DnaDef {
        name: StringFixturator::new_indexed(Empty, self.0.index)
            .next()
            .unwrap(),
        uuid: StringFixturator::new_indexed(Empty, self.0.index)
            .next()
            .unwrap(),
        properties: SerializedBytesFixturator::new_indexed(Empty, self.0.index)
            .next()
            .unwrap(),
        zomes: ZomesFixturator::new_indexed(Empty, self.0.index)
            .next()
            .unwrap(),
    };

    curve Unpredictable DnaDef {
        name: StringFixturator::new_indexed(Unpredictable, self.0.index)
            .next()
            .unwrap(),
        uuid: StringFixturator::new_indexed(Unpredictable, self.0.index)
            .next()
            .unwrap(),
        properties: SerializedBytesFixturator::new_indexed(Unpredictable, self.0.index)
            .next()
            .unwrap(),
        zomes: ZomesFixturator::new_indexed(Unpredictable, self.0.index)
            .next()
            .unwrap(),
    };

    curve Predictable DnaDef {
        name: StringFixturator::new_indexed(Predictable, self.0.index)
            .next()
            .unwrap(),
        uuid: StringFixturator::new_indexed(Predictable, self.0.index)
            .next()
            .unwrap(),
        properties: SerializedBytesFixturator::new_indexed(Predictable, self.0.index)
            .next()
            .unwrap(),
        zomes: ZomesFixturator::new_indexed(Predictable, self.0.index)
            .next()
            .unwrap(),
    };
);

fixturator!(
    DnaFile,
    {
        DnaFile {
            dna: DnaDefFixturator::new(Empty).next().unwrap(),
            dna_hash: DnaHashFixturator::new(Empty).next().unwrap(),
            code: WasmsFixturator::new(Empty).next().unwrap(),
        }
    },
    {
        // align the wasm hashes across the file and def
        let mut zome_name_fixturator = ZomeNameFixturator::new(Unpredictable);
        let wasms = WasmsFixturator::new(Unpredictable).next().unwrap();
        let mut zomes: Zomes = Vec::new();
        for (hash, wasm) in wasms {
            zomes.push((
                zome_name_fixturator.next().unwrap(),
                Zome {
                    wasm_hash: hash.to_owned(),
                },
            ));
        }
        let mut dna_def = DnaDefFixturator::new(Unpredictable).next().unwrap();
        dna_def.zomes = zomes;
        DnaFile {
            dna: dna_def,
            dna_hash: DnaHashFixturator::new(Unpredictable).next().unwrap(),
            code: WasmsFixturator::new(Unpredictable).next().unwrap(),
        }
    },
    {
        // align the wasm hashes across the file and def
        let mut zome_name_fixturator = ZomeNameFixturator::new_indexed(Predictable, self.0.index);
        let wasms = WasmsFixturator::new_indexed(Predictable, self.0.index)
            .next()
            .unwrap();
        let mut zomes: Zomes = Vec::new();
        for (hash, wasm) in wasms {
            zomes.push((
                zome_name_fixturator.next().unwrap(),
                Zome {
                    wasm_hash: hash.to_owned(),
                },
            ));
        }
        let mut dna_def = DnaDefFixturator::new_indexed(Predictable, self.0.index)
            .next()
            .unwrap();
        dna_def.zomes = zomes;
        DnaFile {
            dna: DnaDefFixturator::new_indexed(Predictable, self.0.index)
                .next()
                .unwrap(),
            dna_hash: DnaHashFixturator::new_indexed(Predictable, self.0.index)
                .next()
                .unwrap(),
            code: WasmsFixturator::new_indexed(Predictable, self.0.index)
                .next()
                .unwrap(),
        }
    }
);

fixturator!(
    WasmRibosome;
    constructor fn new(DnaFile);
);

impl Iterator for WasmRibosomeFixturator<curve::Zomes> {
    type Item = WasmRibosome;

    fn next(&mut self) -> Option<Self::Item> {
        // @todo fixturate this
        let dna_file = fake_dna_zomes(
            &StringFixturator::new(Unpredictable).next().unwrap(),
            self.0
                .curve
                .0
                .clone()
                .into_iter()
                .map(|t| (t.into(), t.into()))
                .collect(),
        );
        let ribosome = WasmRibosome::new(dna_file);

        // warm the module cache for each wasm in the ribosome
        for zome in self.0.curve.0.clone() {
            let mut host_context = HostContextFixturator::new(Empty).next().unwrap();
            host_context.zome_name = zome.into();
            ribosome.module(host_context).unwrap();
        }

        self.0.index = self.0.index + 1;

        Some(ribosome)
    }
}

fixturator!(
    EntryHash;
    variants [
        Entry(EntryContentHash)
        Agent(AgentPubKey)
    ];
);

fixturator!(
    Tag; from Bytes;
);

fixturator!(
    LinkAddBuilder;
    constructor fn new(EntryHash, EntryHash, u8, Tag);
);

fixturator!(
    LinkAddBuilderCombo;
    constructor fn new(LinkAddBuilder, HeaderBuilderCommon);
);
pub struct LinkAddBuilderCombo(LinkAddBuilder, HeaderBuilderCommon);

impl LinkAddBuilderCombo {
    fn new(l: LinkAddBuilder, h: HeaderBuilderCommon) -> Self {
        Self(l, h)
    }
}

impl From<LinkAddBuilderCombo> for LinkAdd {
    fn from(l: LinkAddBuilderCombo) -> Self {
        l.0.build(l.1)
    }
}

fixturator!(
    LinkAdd; from LinkAddBuilderCombo;
);

fixturator!(
    LinkRemoveBuilder;
    constructor fn new(HeaderHash);
);

fixturator!(
    LinkRemoveBuilderCombo;
    constructor fn new(LinkRemoveBuilder, HeaderBuilderCommon);
);
pub struct LinkRemoveBuilderCombo(LinkRemoveBuilder, HeaderBuilderCommon);

impl LinkRemoveBuilderCombo {
    fn new(l: LinkRemoveBuilder, h: HeaderBuilderCommon) -> Self {
        Self(l, h)
    }
}

impl From<LinkRemoveBuilderCombo> for LinkRemove {
    fn from(l: LinkRemoveBuilderCombo) -> Self {
        l.0.build(l.1)
    }
}

fixturator!(
    LinkRemove; from LinkRemoveBuilderCombo;
);

fixturator!(
    LinkMetaVal;
    constructor fn new(HeaderHash, EntryHash, Timestamp, u8, Tag);
);

pub struct KnownLinkAdd {
    pub base_address: EntryHash,
    pub target_address: EntryHash,
    pub tag: Tag,
    pub zome_id: ZomeId,
}

pub struct KnownLinkRemove {
    pub link_add_address: holo_hash::HeaderHash,
}

impl Iterator for LinkAddFixturator<KnownLinkAdd> {
    type Item = LinkAdd;
    fn next(&mut self) -> Option<Self::Item> {
        let mut f = LinkAddFixturator::new(Unpredictable).next().unwrap();
        f.base_address = self.0.curve.base_address.clone();
        f.target_address = self.0.curve.target_address.clone();
        f.tag = self.0.curve.tag.clone();
        f.zome_id = self.0.curve.zome_id;
        Some(f)
    }
}

impl Iterator for LinkRemoveFixturator<KnownLinkRemove> {
    type Item = LinkRemove;
    fn next(&mut self) -> Option<Self::Item> {
        let mut f = LinkRemoveFixturator::new(Unpredictable).next().unwrap();
        f.link_add_address = self.0.curve.link_add_address.clone();
        Some(f)
    }
}

impl Iterator for LinkMetaValFixturator<(EntryHash, Tag)> {
    type Item = LinkMetaVal;
    fn next(&mut self) -> Option<Self::Item> {
        let mut f = LinkMetaValFixturator::new(Unpredictable).next().unwrap();
        f.target = self.0.curve.0.clone();
        f.tag = self.0.curve.1.clone();
        Some(f)
    }
}