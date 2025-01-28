use frame::ArbOsFrame;
use revm::{
    context::{BlockEnv, CfgEnv, TxEnv},
    context_interface::result::{EVMError, HaltReason, InvalidTransaction},
    handler::{
        EthExecution, EthHandler, EthPostExecution, EthPreExecution, EthPrecompileProvider,
        EthValidation,
    },
    interpreter::interpreter::{EthInstructionProvider, EthInterpreter},
    Context, Database, Evm,
};

pub mod frame;
pub mod interpreter;
pub mod stylus;
pub mod journal;
pub mod db;

pub type ArbOsExecution<
    CTX,
    ERROR,
    FRAME = ArbOsFrame<
        CTX,
        ERROR,
        EthPrecompileProvider<CTX, ERROR>,
        EthInstructionProvider<EthInterpreter<()>, CTX>,
    >,
> = EthExecution<CTX, ERROR, FRAME>;

pub type ArbOsHandler<
    CTX,
    ERROR,
    VAL = EthValidation<CTX, ERROR>,
    PREEXEC = EthPreExecution<CTX, ERROR>,
    EXEC = ArbOsExecution<CTX, ERROR>,
    POSTEXEC = EthPostExecution<CTX, ERROR, HaltReason>,
> = EthHandler<CTX, ERROR, VAL, PREEXEC, EXEC, POSTEXEC>;

/// ArbOS Error.
pub type ArbOsError<DB> = EVMError<<DB as Database>::Error, InvalidTransaction>;

/// ArbOS Context.
pub type ArbOsContext<DB> = Context<BlockEnv, TxEnv, CfgEnv, DB>;

/// ArbOS EVM type.
pub type ArbOsEvm<DB> =
    Evm<ArbOsError<DB>, ArbOsContext<DB>, ArbOsHandler<ArbOsContext<DB>, ArbOsError<DB>>>;
