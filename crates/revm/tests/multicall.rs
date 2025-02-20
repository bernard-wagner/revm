use alloy_sol_types::sol;
use revm::db::{CacheDB, EmptyDB};
use revm::primitives::{address, hex, keccak256, Address, ResultAndState, TxEnv, TxKind, U256};
use revm::DatabaseRef;
use revm_interpreter::opcode;
use serde_json::Value;

mod common;

// Constants
const MULTICALL_BYTECODE: &[u8] = include_bytes!("assets/multicall.wasm");
const STORAGE_BYTECODE: &[u8] = include_bytes!("assets/storage.wasm");
const MULTICALL_EVM_ARTIFACT: &str = include_str!("assets/MulticallTest.json");

// Contract definition
sol! {
    contract MultiCallTest {
        event Called(address addr, uint8 count, bool success, bytes returnData);
        event Storage(bytes32 slot, bytes32 data, bool write);
        fallback(bytes calldata data) external;
    }
}

// Enums with clear naming
#[derive(Debug, Copy, Clone)]
enum StorageOperation {
    Read,
    Write,
}

#[derive(Debug, Copy, Clone)]
enum CallOperation {
    Call,
    DelegateCall,
    StaticCall,
}

// Helper struct for test environment
struct TestSetup {
    deployer: Address,
    multicall: Address,
    storage: Address,
    multicall_evm: Address,
    db: CacheDB<EmptyDB>,
}

impl TestSetup {
    fn new() -> Self {
        let mut db = CacheDB::new(EmptyDB::new());
        let deployer = address!("Bd770416a3345F91E4B34576cb804a576fa48EB1");

        // Deploy contracts
        let multicall = common::deploy_wasm(&mut db, MULTICALL_BYTECODE.to_vec(), deployer);
        let storage = common::deploy_wasm(&mut db, STORAGE_BYTECODE.to_vec(), deployer);

        // Initialize storage
        let slot = keccak256("some-storage-slot");
        let value = keccak256("some-storage-data");
        db.load_account(storage)
            .unwrap()
            .storage
            .insert(slot.into(), value.into());

        // Deploy EVM version
        let artifact: Value = serde_json::from_str(MULTICALL_EVM_ARTIFACT).unwrap();
        let bytecode = hex::decode(artifact["bytecode"].as_str().unwrap()).unwrap();
        let multicall_evm = common::deploy_solidity(&mut db, bytecode, deployer);

        Self {
            deployer,
            multicall,
            storage,
            multicall_evm,
            db,
        }
    }

    fn prepare_storage_call(
        &self,
        op: StorageOperation,
        slot: &str,
        data: Option<&str>,
    ) -> Vec<u8> {
        let mut calldata = vec![op as u8];
        calldata.extend(keccak256(slot).to_vec());

        calldata.extend(keccak256(data.unwrap_or_default()).to_vec());

        calldata
    }

    fn prepare_multicall(
        &self,
        op: CallOperation,
        to: Address,
        value: Option<U256>,
        data: Vec<u8>,
    ) -> Vec<u8> {
        let mut call_data = vec![op as u8];

        if let CallOperation::Call = op {
            call_data.extend(value.unwrap_or_default().to_be_bytes_vec());
        }

        call_data.extend(to.to_vec());
        call_data.extend(data);

        let mut final_data = vec![0x01]; // Single call
        final_data.extend(U256::from(call_data.len()).to_be_bytes_vec()[28..].to_vec());
        final_data.extend(call_data);

        final_data
    }

    fn prepare_multicall_evm(
        &self,
        op: CallOperation,
        to: Address,
        value: Option<U256>,
        data: Vec<u8>,
    ) -> Vec<u8> {
        let mut call_data = vec![];
        match op {
            CallOperation::Call => call_data.push(opcode::CALL),
            CallOperation::DelegateCall => call_data.push(opcode::DELEGATECALL),
            CallOperation::StaticCall => call_data.push(opcode::STATICCALL),
        }

        if let CallOperation::Call = op {
            call_data.extend(value.unwrap_or_default().to_be_bytes_vec());
        }

        call_data.extend(to.to_vec());
        call_data.extend(data);

        call_data
    }

    fn execute(&mut self, to: Address, data: Vec<u8>) -> revm::primitives::ExecutionResult {
        let mut evm = revm::Evm::builder()
            .with_db(&mut self.db)
            .modify_tx_env(|tx: &mut TxEnv| {
                tx.caller = self.deployer;
                tx.transact_to = TxKind::Call(to);
                tx.data = data.into();
                tx.gas_limit = 1_000_000_000;
            })
            .build();

        let tx = evm.transact().unwrap();

        tx.result
    }

    fn execute_commit(&mut self, to: Address, data: Vec<u8>) -> revm::primitives::ExecutionResult {
        let mut evm = revm::Evm::builder()
            .with_db(&mut self.db)
            .modify_tx_env(|tx: &mut TxEnv| {
                tx.caller = self.deployer;
                tx.transact_to = TxKind::Call(to);
                tx.data = data.into();
                tx.gas_limit = 1_000_000_000;
            })
            .build();

        evm.transact_commit().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_direct_storage_read() {
        let mut setup = TestSetup::new();

        let call_data =
            setup.prepare_storage_call(StorageOperation::Read, "some-storage-slot", None);

        let result = setup.execute(setup.storage, call_data);

        assert!(result.is_success());
        assert_eq!(
            result.output().unwrap().to_vec(),
            keccak256("some-storage-data").to_vec()
        );
    }

    #[test]
    fn test_multicall_storage_read() {
        let mut setup = TestSetup::new();

        let storage_call =
            setup.prepare_storage_call(StorageOperation::Read, "some-storage-slot", None);

        let multicall =
            setup.prepare_multicall(CallOperation::Call, setup.storage, None, storage_call);

        let result = setup.execute(setup.multicall, multicall);

        assert!(result.is_success());
        assert_eq!(
            result.output().unwrap().to_vec(),
            keccak256("some-storage-data").to_vec()
        );
    }

    #[test]
    fn test_multicall_storage_write() {
        let mut setup = TestSetup::new();

        let storage_call = setup.prepare_storage_call(
            StorageOperation::Write,
            "some-storage-slot",
            Some("new-storage-value"),
        );

        let multicall =
            setup.prepare_multicall(CallOperation::Call, setup.storage, None, storage_call);

        setup.execute_commit(setup.multicall, multicall);

        let slot = keccak256("some-storage-slot");
        let stored_value = setup
            .db
            .load_account(setup.storage)
            .unwrap()
            .storage
            .get(&slot.into())
            .unwrap();

        assert_eq!(
            stored_value.to_be_bytes_vec(),
            keccak256("new-storage-value").to_vec()
        );
    }

    #[test]
    fn test_static_call_write_protection() {
        let mut setup = TestSetup::new();

        let storage_call = setup.prepare_storage_call(
            StorageOperation::Write,
            "some-storage-slot",
            Some("new-storage-value"),
        );

        let multicall =
            setup.prepare_multicall(CallOperation::StaticCall, setup.storage, None, storage_call);

        let result = setup.execute_commit(setup.multicall, multicall);
        let output = String::from_utf8_lossy(result.output().unwrap());

        assert!(output.contains("WriteProtection"));
    }

    #[test]
    fn test_delegatecall_storage_context() {
        let mut setup = TestSetup::new();

        // Set up multicall contract storage
        let slot = keccak256("some-storage-slot");
        setup
            .db
            .load_account(setup.multicall)
            .unwrap()
            .storage
            .insert(slot.into(), keccak256("multicall-storage-value").into());

        let storage_call =
            setup.prepare_storage_call(StorageOperation::Read, "some-storage-slot", None);

        let multicall = setup.prepare_multicall(
            CallOperation::DelegateCall,
            setup.storage,
            None,
            storage_call,
        );

        let result = setup.execute_commit(setup.multicall, multicall);

        assert!(result.is_success());
        assert_eq!(
            result.output().unwrap().to_vec(),
            keccak256("multicall-storage-value").to_vec()
        );
    }

    //#[test]
    // fn test_multicall_to_evm() {
    //     let mut setup = TestSetup::new();

    //     let slot = keccak256("some-storage-slot");

    //     let storage_call = setup.prepare_storage_call_evm(
    //         StorageOperation::Write,
    //         "some-storage-slot",
    //         Some("new-storage-value")
    //     );

    //     let multicall = setup.prepare_multicall(
    //         CallOperation::Call,
    //         setup.multicall_evm,
    //         None,
    //         storage_call
    //     );

    //     let result = setup.execute(setup.multicall, multicall);

    //     println!("{:?}", result);

    //     assert!(result.is_success());
    //     let stored_value = setup.db
    //     .load_account(setup.multicall_evm)
    //     .unwrap()
    //     .storage
    //     .get(&slot.into())
    //     .unwrap();

    //     assert_eq!(
    //         stored_value.to_be_bytes_vec(),
    //         keccak256("new-storage-value").to_vec()
    //     );
    // }
}
