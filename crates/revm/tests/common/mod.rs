use revm::{db::{CacheDB, EmptyDB}, primitives::{address, bytes::Bytes, B256, keccak256, AccountInfo, Address, Bytecode, SpecId::LATEST, TxEnv, TxKind, U256}, Database, DatabaseRef, STYLUS_MAGIC_BYTES};

pub(crate) const DEPLOYER: Address = address!("Bd770416a3345F91E4B34576cb804a576fa48EB1");

pub(crate) fn setup_simple_test(db: &mut CacheDB<EmptyDB>) {
    let mut info = AccountInfo::default();
    info.balance = U256::from(1e18);
    db.insert_account_info(DEPLOYER, info);
}

pub(crate) fn deploy_wasm<T: DatabaseRef>(db: &mut CacheDB<T>, bytecode: Vec<u8>, deployer: Address) -> Address {
    let nonce = db.accounts.get(&deployer).and_then(|acc| Some(acc.info.nonce)).unwrap_or_default();   
    let deployed_address = deployer.create(nonce);

    let bytecode = wasm_contract_init_code(bytecode);

    let evm = revm::Evm::builder()
        .with_db(db)
        .with_spec_id(LATEST)
        .modify_tx_env(|tx: &mut TxEnv| {
            tx.caller = deployer;
            tx.transact_to = TxKind::Create;
            tx.data = bytecode.into();
        }
    );

    match evm.build().transact_commit() {
        Ok(res) => res,
        Err(_) => {
            panic!("Failed to deploy contract");
        }
    };

  

    deployed_address
}

pub(crate) fn deploy_solidity(db: &mut CacheDB<EmptyDB>, bytecode: Vec<u8>, deployer: Address) -> Address {
    let nonce = db.accounts.get(&deployer).and_then(|acc| Some(acc.info.nonce)).unwrap_or_default();   
    let deployed_address = deployer.create(nonce);

    let evm = revm::Evm::builder()
        .with_db(db)
        .with_spec_id(LATEST)
        .modify_tx_env(|tx: &mut TxEnv| {
            tx.caller = deployer;
            tx.transact_to = TxKind::Create;
            tx.data = bytecode.into();
        }
    );

    deployed_address
}

pub(crate) fn wasm_contract_init_code(bytecode: Vec<u8>) ->  Vec<u8> {
    let mut bytecode = [
        Bytes::from(STYLUS_MAGIC_BYTES),
        Bytes::from(bytecode),
    ]
    .concat();

    let mut deploy = vec![];
    deploy.push(revm::interpreter::opcode::PUSH32);
    deploy.append(&mut U256::from(bytecode.len()).to_be_bytes_vec());
    deploy.push(revm::interpreter::opcode::DUP1);
    deploy.push(revm::interpreter::opcode::PUSH1);
    deploy.push(42);
    deploy.push(revm::interpreter::opcode::PUSH1);
    deploy.push(0);
    deploy.push(revm::interpreter::opcode::CODECOPY);
    deploy.push(revm::interpreter::opcode::PUSH1);
    deploy.push(0);
    deploy.push(revm::interpreter::opcode::RETURN);
    deploy.append(&mut bytecode);
    deploy
}
