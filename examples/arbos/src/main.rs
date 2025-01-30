//! Optimism-specific constants, types, and helpers.
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use std::convert::Infallible;

use anyhow::{anyhow, bail};
use database::CacheDB;
use revm::{
    context::{BlockEnv, CfgEnv, Context, TxEnv},
    context_interface::{
        result::{EVMError, InvalidTransaction},
        DatabaseGetter,
    },
    database_interface::{EmptyDB, EmptyDBTyped},
    handler::{EthPostExecution, EthPreExecution, EthValidation},
    primitives::{address, hex, keccak256, Bytes, TxKind, U256},
    state::Bytecode,
    Database, Error,
};
use revm_arbos::{interpreter::STYLUS_MAGIC_BYTES, ArbOsEvm, ArbOsExecution, ArbOsHandler};

use alloy_sol_types::sol;
use alloy_sol_types::SolCall;

/// Load storage from slot zero to memory
const RUNTIME_BYTECODE: &[u8] = include_bytes!("../stylus_hello_world.wasm");

const SOLIDITY_BYTECODE: &[u8] = hex!("608060405234801561000f575f80fd5b5060043610610060575f3560e01c80631042c9f31461006457806326929eb61461008d5780633fb5c1cb146100a057806380bbeb57146100b45780638381f58a146100c7578063d09de08a146100dd575b5f80fd5b610077610072366004610252565b6100e5565b60405161008491906102dd565b60405180910390f35b61007761009b366004610252565b610190565b6100b26100ae366004610312565b5f55565b005b6100776100c2366004610252565b6101e6565b6100cf5f5481565b604051908152602001610084565b6100b261023d565b60605f80856001600160a01b03168585604051610103929190610329565b5f60405180830381855afa9150503d805f811461013b576040519150601f19603f3d011682016040523d82523d5f602084013e610140565b606091505b5091509150816101875760405162461bcd60e51b815260206004820152600e60248201526d119bdc9dd85c990819985a5b195960921b604482015260640160405180910390fd5b95945050505050565b60605f80856001600160a01b031685856040516101ae929190610329565b5f60405180830381855af49150503d805f811461013b576040519150601f19603f3d011682016040523d82523d5f602084013e610140565b60605f80856001600160a01b03168585604051610204929190610329565b5f604051808303815f865af19150503d805f811461013b576040519150601f19603f3d011682016040523d82523d5f602084013e610140565b5f8054908061024b83610338565b9190505550565b5f805f60408486031215610264575f80fd5b83356001600160a01b038116811461027a575f80fd5b9250602084013567ffffffffffffffff811115610295575f80fd5b8401601f810186136102a5575f80fd5b803567ffffffffffffffff8111156102bb575f80fd5b8660208284010111156102cc575f80fd5b939660209190910195509293505050565b602081525f82518060208401528060208501604085015e5f604082850101526040601f19601f83011684010191505092915050565b5f60208284031215610322575f80fd5b5035919050565b818382375f9101908152919050565b5f6001820161035557634e487b7160e01b5f52601160045260245ffd5b506001019056fea2646970667358221220022dc685d2ea22230c54c8fb51efaa848bbbeef52ce2b9642e050b056ff7d2b364736f6c634300081a0033").as_slice();

// generate abi for the calldata from the human readable interface
sol! {
    function number() external view returns (uint256);
    function setNumber(uint256 new_number) external;
    function setNumberRevert(uint256 new_number) external;
    function forwardTo(address to, bytes calldata data) external returns (bytes memory);

    function forwardStatic(address to, bytes calldata data) external view returns (bytes memory);

    function forwardDelegate(address to, bytes calldata data) external returns (bytes memory);
}

pub type ArbOsError<DB> = EVMError<<DB as Database>::Error, InvalidTransaction>;

type Db = CacheDB<EmptyDB>;

type Ctx = Context<BlockEnv, TxEnv, CfgEnv, CacheDB<EmptyDBTyped<Infallible>>>;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let stylus_address = address!("Bd770416a3345F91E4B34576cb804a576fa48EB1");
    let solidity_address = address!("Bd770416a3345F91E4B34576cb804a576fa48EB2");
    let param = 1337;

    let db = CacheDB::<EmptyDB>::default();

    let validation: EthValidation<Ctx, Error<Db>> = EthValidation::new();

    let mut evm = ArbOsEvm::new(
        Context::builder().with_db(db),
        ArbOsHandler::new(
            validation,
            EthPreExecution::new(),
            ArbOsExecution::new(),
            EthPostExecution::new(),
        ),
    );

    //println!("name: {:?}", evm.handler.execution.name());
    {
        let bytecode =
            Bytes::from([STYLUS_MAGIC_BYTES.clone(), Bytes::from(RUNTIME_BYTECODE)].concat());

        evm.context.borrow_mut().db().insert_account_info(
            stylus_address,
            revm::state::AccountInfo {
                balance: U256::from(0),
                nonce: 0,
                code_hash: keccak256(bytecode.clone()),
                code: Some(Bytecode::new_legacy(bytecode)),
            },
        );
    }

    {
        let bytecode = Bytes::from(SOLIDITY_BYTECODE);
        evm.context.borrow_mut().db().insert_account_info(
            solidity_address,
            revm::state::AccountInfo {
                balance: U256::from(0),
                nonce: 0,
                code_hash: keccak256(bytecode.clone()),
                code: Some(Bytecode::new_legacy(bytecode)),
            },
        );
    }

    // evm.context.modify_tx(|tx| {
    //     tx.data = setNumberCall::new((U256::from(param),)).abi_encode().into();
    //     tx.kind = TxKind::Call(stylus_address);
    // });

    // evm.context.modify_tx(|tx| {
    //     tx.data = forwardToCall::new(
    //         (stylus_address, setNumberCall::new((U256::from(param),)).abi_encode().into(), )
    //     ).abi_encode().into();
    //     tx.kind = TxKind::Call(stylus_address);
    // });

    // evm.context.modify_tx(|tx| {
    //     tx.data = forwardToCall::new((
    //         solidity_address,
    //         forwardToCall::new((
    //             stylus_address,
    //             setNumberCall::new((U256::from(param),)).abi_encode().into(),
    //         ))
    //         .abi_encode()
    //         .into(),
    //     ))
    //     .abi_encode()
    //     .into();
    //     tx.kind = TxKind::Call(stylus_address);
    // });

    evm.context.borrow_mut().modify_tx(|tx| {
        tx.data = forwardToCall::new((
            solidity_address,
            forwardToCall::new((
                stylus_address,
                setNumberCall::new((U256::from(param),)).abi_encode().into(),
            ))
            .abi_encode()
            .into(),
        ))
        .abi_encode()
        .into();
        tx.kind = TxKind::Call(solidity_address);
    });

    let result = evm.transact()?;

    let Some(storage0) = result
        .state
        .get(&stylus_address)
        .ok_or_else(|| anyhow!("Contract not found"))?
        .storage
        .get::<U256>(&Default::default())
    else {
        bail!("Failed to write storage in the init code: {result:#?}");
    };

    println!("storage U256(0) at {stylus_address}:  {storage0:#?}");
    assert_eq!(storage0.present_value(), param.try_into()?, "{result:#?}");
    Ok(())
}
