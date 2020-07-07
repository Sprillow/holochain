use super::*;

mock! {
    pub MetadataBuf
    {
        fn get_links<'a>(&self, key: &'a LinkMetaKey<'a>) -> DatabaseResult<Vec<LinkMetaVal>>;
        fn add_link(&mut self, link_add: LinkAdd) -> DatabaseResult<()>;
        fn remove_link(&mut self, link_remove: LinkRemove, base: &EntryHash, zome_id: ZomeId, tag: LinkTag) -> DatabaseResult<()>;
        fn sync_add_create(&self, create: header::EntryCreate) -> DatabaseResult<()>;
        fn sync_register_header(&mut self, new_entry_header: NewEntryHeader) -> DatabaseResult<()>;
        fn sync_register_activity(
            &mut self,
            header: Header,
        ) -> DatabaseResult<()>;
        fn sync_register_update(&mut self, update: header::EntryUpdate, entry: Option<EntryHash>) -> DatabaseResult<()>;
        fn sync_register_delete_on_entry(&self, delete: header::ElementDelete, entry_hash: EntryHash) -> DatabaseResult<()>;
        fn sync_register_delete_on_header(&mut self, delete: header::ElementDelete) -> DatabaseResult<()>;
        fn get_dht_status(&self, entry_hash: &EntryHash) -> DatabaseResult<EntryDhtStatus>;
        fn get_canonical_entry_hash(&self, entry_hash: EntryHash) -> DatabaseResult<EntryHash>;
        fn get_canonical_header_hash(&self, header_hash: HeaderHash) -> DatabaseResult<HeaderHash>;
        fn get_headers(
            &self,
            entry_hash: EntryHash,
        ) -> DatabaseResult<Box<dyn FallibleIterator<Item = HeaderHash, Error = DatabaseError>>>;
        fn get_activity(
            &self,
            header_hash: AgentPubKey,
        ) -> DatabaseResult<Box<dyn FallibleIterator<Item = HeaderHash, Error = DatabaseError>>>;
        fn get_updates(
            &self,
            hash: AnyDhtHash,
        ) -> DatabaseResult<Box<dyn FallibleIterator<Item = HeaderHash, Error = DatabaseError>>>;
        fn get_deletes(
            &self,
            entry_or_new_entry_header: AnyDhtHash,
        ) -> DatabaseResult<Box<dyn FallibleIterator<Item = HeaderHash, Error = DatabaseError>>>;
    }
}

#[async_trait::async_trait]
impl MetadataBufT for MockMetadataBuf {
    fn get_links<'a>(&self, key: &'a LinkMetaKey) -> DatabaseResult<Vec<LinkMetaVal>> {
        self.get_links(key)
    }

    fn get_canonical_entry_hash(&self, entry_hash: EntryHash) -> DatabaseResult<EntryHash> {
        self.get_canonical_entry_hash(entry_hash)
    }

    fn get_dht_status(&self, entry_hash: &EntryHash) -> DatabaseResult<EntryDhtStatus> {
        self.get_dht_status(entry_hash)
    }

    fn get_canonical_header_hash(&self, header_hash: HeaderHash) -> DatabaseResult<HeaderHash> {
        self.get_canonical_header_hash(header_hash)
    }

    fn get_headers(
        &self,
        entry_hash: EntryHash,
    ) -> DatabaseResult<Box<dyn FallibleIterator<Item = HeaderHash, Error = DatabaseError> + '_>>
    {
        self.get_headers(entry_hash)
    }

    fn get_activity(
        &self,
        agent_pubkey: AgentPubKey,
    ) -> DatabaseResult<Box<dyn FallibleIterator<Item = HeaderHash, Error = DatabaseError> + '_>>
    {
        self.get_activity(agent_pubkey)
    }

    fn get_updates(
        &self,
        hash: AnyDhtHash,
    ) -> DatabaseResult<Box<dyn FallibleIterator<Item = HeaderHash, Error = DatabaseError> + '_>>
    {
        self.get_updates(hash)
    }

    fn get_deletes(
        &self,
        entry_or_new_entry_header: AnyDhtHash,
    ) -> DatabaseResult<Box<dyn FallibleIterator<Item = HeaderHash, Error = DatabaseError> + '_>>
    {
        self.get_deletes(entry_or_new_entry_header)
    }

    async fn add_link(&mut self, link_add: LinkAdd) -> DatabaseResult<()> {
        self.add_link(link_add)
    }

    fn remove_link(
        &mut self,
        link_remove: LinkRemove,
        base: &EntryHash,
        zome_id: ZomeId,
        tag: LinkTag,
    ) -> DatabaseResult<()> {
        self.remove_link(link_remove, base, zome_id, tag)
    }

    async fn register_header(&mut self, new_entry_header: NewEntryHeader) -> DatabaseResult<()> {
        self.sync_register_header(new_entry_header)
    }

    async fn register_activity(&mut self, header: Header) -> DatabaseResult<()> {
        self.sync_register_activity(header)
    }

    async fn register_update(
        &mut self,
        update: header::EntryUpdate,
        entry: Option<EntryHash>,
    ) -> DatabaseResult<()> {
        self.sync_register_update(update, entry)
    }
    async fn register_delete_on_entry(
        &mut self,
        delete: header::ElementDelete,
        entry_hash: EntryHash,
    ) -> DatabaseResult<()> {
        self.sync_register_delete_on_entry(delete, entry_hash)
    }
    async fn register_delete_on_header(
        &mut self,
        delete: header::ElementDelete,
    ) -> DatabaseResult<()> {
        self.sync_register_delete_on_header(delete)
    }
}