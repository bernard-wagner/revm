
use common::{deploy_wasm, setup_simple_test, wasm_contract_init_code, DEPLOYER};
use revm::arbos::STYLUS_MAGIC_BYTES;
use revm::db::{CacheDB, EmptyDB};
use revm::primitives::bytes::Bytes;
use revm::primitives::{address, ExecutionResult, TxEnv, TxKind, B256, U256};
const CREATE_BYTECODE: &[u8] = include_bytes!("assets/create.wasm");
const STORAGE_BYTECODE: &[u8] = include_bytes!("assets/storage.wasm");

mod common; 

fn create_test(balance: U256, endowment: U256) {
    let mut db = CacheDB::new(EmptyDB::new());
   
    setup_simple_test(&mut db);

    let create_address = deploy_wasm(&mut db, CREATE_BYTECODE.to_vec(), DEPLOYER);

    db.load_account(create_address).unwrap().info.balance = balance;
    
    let code = wasm_contract_init_code(STORAGE_BYTECODE.to_vec());

    let expected_address = create_address.create(1);

    let mut data: Vec<u8> = vec![];
    data.extend([0x01]); 
    data.extend(endowment.to_be_bytes_vec());
    data.extend(code);

    {
        let evm = revm::Evm::builder()
            .with_db(&mut db)
            .modify_tx_env(|tx: &mut TxEnv| {
                tx.caller = DEPLOYER;
                tx.transact_to = TxKind::Call(create_address);
                tx.data = data.into();
                tx.gas_limit = 1e9 as u64;
            }
        );
        let result = evm.build().transact_commit().unwrap();

        assert!(result.is_success());

        let logs = result.logs();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].address, create_address);
        assert_eq!(logs[0].topics().len(), 1);
        assert_eq!(logs[0].topics()[0], expected_address.into_word());
    }

    let account_info = db.accounts.get(&expected_address).unwrap().info.clone();
    
    assert_eq!(account_info.balance, endowment);
    assert_eq!(account_info.code.unwrap().original_bytes().to_vec().len(),  [
        Bytes::from(STYLUS_MAGIC_BYTES),
        Bytes::from(STORAGE_BYTECODE),
    ]
    .concat().len());
}

#[test]
pub fn create_with_no_endowment() {
    create_test(U256::ZERO, U256::ZERO);
}

#[test]
pub fn create_with_endowment() {
    create_test(U256::from(0.5e18),U256::from(0.5e18));
}

#[test]
pub fn create_with_endowment_exceeds_balance() {
    let endowment = U256::from(1.5e18);
    let mut db = CacheDB::new(EmptyDB::new());
   
    setup_simple_test(&mut db);

    let create_address = deploy_wasm(&mut db, CREATE_BYTECODE.to_vec(), DEPLOYER);

    db.load_account(create_address).unwrap().info.balance = U256::from(0.5e18);
    
    let code = wasm_contract_init_code(STORAGE_BYTECODE.to_vec());

    let mut data: Vec<u8> = vec![];
    data.extend([0x01]); 
    data.extend(endowment.to_be_bytes_vec());
    data.extend(code);

    {
        let evm = revm::Evm::builder()
            .with_db(&mut db)
            .modify_tx_env(|tx: &mut TxEnv| {
                tx.caller = DEPLOYER;
                tx.transact_to = TxKind::Call(create_address);
                tx.data = data.into();
            }
        );
        let result = evm.build().transact_commit().unwrap();

        match result {
            ExecutionResult::Revert{output, ..} => {               
                assert_eq!(output, Bytes::new());
            },
            _ => panic!("Expected revert: {:?}", result),
        }
    }
}

#[test]
pub fn create_2() {
    let mut db = CacheDB::new(EmptyDB::new());
   
    let deployer = address!("Bd770416a3345F91E4B34576cb804a576fa48EB1");

    let deployed_address = common::deploy_wasm(&mut db, CREATE_BYTECODE.to_vec(), deployer);

    let kind: u8 = 0x02; // CREATE2
    let endowment = U256::from(0);
    let salt = B256::from(U256::from(1234));

    let code = wasm_contract_init_code(STORAGE_BYTECODE.to_vec());

    let mut data: Vec<u8> = vec![];
    data.extend([kind]);
    data.extend(endowment.to_be_bytes_vec());
    data.extend(salt.to_vec());
    data.extend(code.clone());

    let expected_address = deployed_address.create2_from_code(salt, code.clone());
    {
        let evm = revm::Evm::builder()
            .with_db(&mut db)
            .modify_tx_env(|tx: &mut TxEnv| {
                tx.caller = deployer;
                tx.transact_to = TxKind::Call(deployed_address);
                tx.data = data.into();
            }
        );

        let result = evm.build().transact_commit().unwrap();

        assert!(result.is_success());
 
        let logs = result.logs();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].address, deployed_address);
        assert_eq!(logs[0].topics().len(), 1);
        assert_eq!(logs[0].topics()[0], expected_address.into_word());
    }

    let account_code = db.accounts.get(&expected_address).unwrap().info.code.clone();
    assert_eq!(account_code.unwrap().original_bytes().to_vec(),  [
        Bytes::from(STYLUS_MAGIC_BYTES),
        Bytes::from(STORAGE_BYTECODE),
    ]
    .concat());
}

