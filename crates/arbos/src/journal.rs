use alloy_primitives::{Address, B256, U256};
use revm::{
    context_interface::{Journal, JournalCheckpoint}, interpreter::{SStoreResult, SelfDestructResult, StateLoad}, precompile::{HashSet, Log}, primitives::hash_map::HashMap, specification::hardfork::SpecId, state::{Account, Bytecode, EvmState}, Database, JournaledState
};

use crate::db::WasmTarget;

pub trait ArbOsJournal {
    fn activate_wasm(&mut self, module_hash: B256, asm_map: HashMap<WasmTarget, Vec<u8>>);
    fn try_get_activated_asm(&self, target: WasmTarget, module_hash: B256) -> Option<Vec<u8>>;
    fn get_stylus_pages(&self) -> (u16, u16);
    fn add_stylus_pages(&mut self, new: u16) -> (u16, u16);
}

pub struct ArbOsJournaledState<DB> {
    inner: JournaledState<DB>,
    activated_wasms: HashMap<B256, HashMap<WasmTarget, Vec<u8>>>,
    open_wasm_pages: u16,
    ever_wasm_pages: u16,
}

pub static STYLUS_DISCRIMINANT: [u8; 3] = [0xEF, 0xF0, 0x00];

impl<DB: Database>  ArbOsJournaledState<DB> {
    pub fn new(spec: SpecId, database: DB) -> ArbOsJournaledState<DB> {
        Self {
            inner: JournaledState::new(spec, database),
            activated_wasms: HashMap::new(),
            open_wasm_pages: 0,
            ever_wasm_pages: 0,
        }
    }

    pub fn is_stylus_program(code: &[u8]) -> bool {
        if code.len() < STYLUS_DISCRIMINANT.len() + 1 {
            return false;
        }
        code[..3] == STYLUS_DISCRIMINANT
    }

    pub fn strip_stylus_prefix(code: &[u8]) -> Result<(&[u8], u8), String> {
        if !Self::is_stylus_program(code) {
            return Err("specified bytecode is not a Stylus program".to_string());
        }
        Ok((&code[4..], code[3]))
    }

    pub fn new_stylus_prefix(dictionary: u8) -> Vec<u8> {
        let mut prefix = STYLUS_DISCRIMINANT.to_vec();
        prefix.push(dictionary);
        prefix
    }
}

impl<DB: Database> ArbOsJournal for ArbOsJournaledState<DB> {

    fn activate_wasm(&mut self, module_hash: B256, asm_map: HashMap<WasmTarget, Vec<u8>>) {
        self.activated_wasms.entry(module_hash).or_insert(asm_map);
    }

    fn try_get_activated_asm(&self, target: WasmTarget, module_hash: B256) -> Option<Vec<u8>> {
        self.activated_wasms.get(&module_hash).and_then(|map| map.get(&target).cloned())
    }

    fn get_stylus_pages(&self) -> (u16, u16) {
        (self.open_wasm_pages, self.ever_wasm_pages)
    }

    fn add_stylus_pages(&mut self, new: u16) -> (u16, u16) {
        let (open, ever) = self.get_stylus_pages();
        self.open_wasm_pages = open.saturating_add(new);
        self.ever_wasm_pages = std::cmp::max(ever, self.open_wasm_pages);
        (open, ever)
    }
}

impl<DB: Database> Journal for ArbOsJournaledState<DB> {
    type Database = DB;
    type FinalOutput = (EvmState, Vec<Log>);
    fn new(database: Self::Database) -> Self {
       Self::new(SpecId::LATEST, database)
    }

    fn db_ref(&self) -> &Self::Database {
        self.inner.db_ref()
    }

    fn db(&mut self) -> &mut Self::Database {
        self.inner.db()
    }

    fn sload(&mut self, address: Address, key: U256) -> Result<StateLoad<U256>, <Self::Database as Database>::Error> {
        self.inner.sload(address, key)
    }

    fn sstore(&mut self, address: Address, key: U256, value: U256) -> Result<StateLoad<SStoreResult>, <Self::Database as Database>::Error> {
        self.inner.sstore(address, key, value)
    }

    fn tload(&mut self, address: Address, key: U256) -> U256 {
        self.inner.tload(address, key)
    }

    fn tstore(&mut self, address: Address, key: U256, value: U256) {
        self.inner.tstore(address, key, value)
    }

    fn log(&mut self, log: Log) {
        self.inner.log(log)
    }

    fn selfdestruct(&mut self, address: Address, target: Address) -> Result<StateLoad<SelfDestructResult>, <Self::Database as Database>::Error> {
        self.inner.selfdestruct(address, target)
    }

    fn warm_account_and_storage(&mut self, address: Address, storage_keys: impl IntoIterator<Item = U256>) -> Result<(), <Self::Database as Database>::Error> {
        self.inner.warm_account_and_storage(address, storage_keys)
    }

    fn warm_account(&mut self, address: Address) {
        self.inner.warm_account(address)
    }

    fn warm_precompiles(&mut self, addresses: HashSet<Address>) {
        self.inner.warm_precompiles(addresses)
    }

    fn precompile_addresses(&self) -> &HashSet<Address> {
        self.inner.precompile_addresses()
    }

    fn set_spec_id(&mut self, spec_id: SpecId) {
        self.inner.set_spec_id(spec_id)
    }

    fn touch_account(&mut self, address: Address) {
        self.inner.touch_account(address)
    }

    fn transfer(&mut self, from: &Address, to: &Address, balance: U256) -> Result<Option<revm::context_interface::journaled_state::TransferError>, <Self::Database as Database>::Error> {
        self.inner.transfer(from, to, balance)
    }

    fn inc_account_nonce(&mut self, address: Address) -> Result<Option<u64>, <Self::Database as Database>::Error> {
        self.inner.inc_account_nonce(address)
    }

    fn load_account(&mut self, address: Address) -> Result<StateLoad<&mut Account>, <Self::Database as Database>::Error> {
        self.inner.load_account(address)
    }

    fn load_account_code(&mut self, address: Address) -> Result<StateLoad<&mut Account>, <Self::Database as Database>::Error> {
        self.inner.load_account_code(address)
    }

    fn load_account_delegated(&mut self, address: Address) -> Result<StateLoad<revm::context_interface::journaled_state::AccountLoad>, <Self::Database as Database>::Error> {
        self.inner.load_account_delegated(address)
    }

    fn set_code_with_hash(&mut self, address: Address, code: Bytecode, hash: B256) {
        self.inner.set_code_with_hash(address, code, hash)
    }

    fn clear(&mut self) {
        self.inner.clear()
    }

    fn checkpoint(&mut self) -> JournalCheckpoint {
        self.inner.checkpoint()
    }

    fn checkpoint_commit(&mut self) {
        self.inner.checkpoint_commit()
    }

    fn checkpoint_revert(&mut self, checkpoint: JournalCheckpoint) {
        self.inner.checkpoint_revert(checkpoint)
    }

    fn create_account_checkpoint(&mut self, caller: Address, address: Address, balance: U256, spec_id: SpecId) -> Result<JournalCheckpoint, revm::context_interface::journaled_state::TransferError> {
        self.inner.create_account_checkpoint(caller, address, balance, spec_id)
    }

    fn depth(&self) -> usize {
        self.inner.depth()
    }

    fn finalize(&mut self) -> Result<Self::FinalOutput, <Self::Database as Database>::Error> {
        self.inner.finalize()
    }
    

}
