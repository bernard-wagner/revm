use alloy_sol_types::{sol, SolEvent, SolValue};

use revm::db::{CacheDB, EmptyDB};
use revm::primitives::alloy_primitives::IntoLogData;
use revm::primitives::{address, TxEnv, TxKind, U256};

mod common;

const LOGS_BYTECODE: &[u8] = include_bytes!("assets/log.wasm");

sol! {
    event HelloFromStylus(address indexed some_address, uint256 some_number, bytes some_data);
}

#[test]
pub fn emit_logs() {
    let mut db = CacheDB::new(EmptyDB::new());

    let deployer = address!("Bd770416a3345F91E4B34576cb804a576fa48EB1");

    let deployed_address = common::deploy_wasm(&mut db, LOGS_BYTECODE.to_vec(), deployer);

    let expected_log = HelloFromStylus {
        some_address: address!("Bd770416a3345F91E4B34576cb804a576fa48EB2"),
        some_number: U256::from(1337),
        some_data: revm_precompile::Bytes::from("0xdeadbeef"),
    };

    let expected = expected_log.clone().into_log_data();

    let mut data: Vec<u8> = vec![];
    data.extend([expected.topics().len() as u8]);
    data.extend(expected.topics().abi_encode_packed());
    data.extend(expected.data);

    let mut evm = revm::Evm::builder()
        .with_db(db)
        .modify_tx_env(|tx: &mut TxEnv| {
            tx.caller = deployer;
            tx.transact_to = TxKind::Call(deployed_address);
            tx.data = data.into();
        })
        .build();

    let result = evm.transact().unwrap();

    assert!(result.result.is_success());
    let logs = result.result.logs();

    let log = HelloFromStylus::decode_log(&logs[0], true).unwrap();

    assert_eq!(logs[0].address, deployed_address);
    assert_eq!(log.some_address, expected_log.some_address);
    assert_eq!(log.some_number, expected_log.some_number);
    assert_eq!(log.some_data, expected_log.some_data);
}
