use std::sync::{Arc, Mutex};
use std::{cell::RefCell, ops::Deref, rc::Rc};

use revm::context_interface::{BlockGetter, CfgGetter};
use revm::interpreter::SharedMemory;
use revm::primitives::{bytes, Bytes};
use revm::{
    interpreter::{
        interpreter::{EthInterpreter, ExtBytecode},
        table::CustomInstruction,
        Host, InputsImpl, Interpreter, InterpreterAction, InterpreterTypes, MemoryGetter,
    },
    specification::hardfork::SpecId,
};

use crate::stylus::handler::FrameCreateFunc;
use crate::stylus::interpreter::StylusInterpreter;

pub static STYLUS_MAGIC_BYTES: Bytes = bytes!("eff00000");

enum InternalInterpreter<CTX, EXT: Default = (), MG: MemoryGetter = SharedMemory> {
    Arb(StylusInterpreter<CTX>),
    Eth(Box<Interpreter<EthInterpreter<EXT, MG>>>),
}

pub struct ArbInterpreter<CTX, EXT: Default = (), MG: MemoryGetter = SharedMemory> {
    inner: InternalInterpreter<CTX, EXT, MG>,
}

impl<CTX, EXT: Default, MG: MemoryGetter> ArbInterpreter<CTX, EXT, MG> {
    pub fn new(
        memory: Rc<RefCell<MG>>,
        bytecode: ExtBytecode,
        inputs: InputsImpl,
        is_static: bool,
        is_eof_init: bool,
        spec_id: SpecId,
        gas_limit: u64,
    ) -> Self {
        let inner = if let revm::state::Bytecode::LegacyAnalyzed(legacy_bytecode) = bytecode.deref()
        {
            if let Some(bytecode) = legacy_bytecode
                .original_bytes()
                .strip_prefix(&STYLUS_MAGIC_BYTES[..])
            {
                InternalInterpreter::Arb(StylusInterpreter::new(
                    Bytes::copy_from_slice(bytecode),
                    inputs,
                    is_static,
                    gas_limit,
                ))
            } else {
                InternalInterpreter::Eth(Box::new(Interpreter::new(
                    memory,
                    bytecode,
                    inputs,
                    is_static,
                    is_eof_init,
                    spec_id,
                    gas_limit,
                )))
            }
        } else {
            InternalInterpreter::Eth(Box::new(Interpreter::new(
                memory,
                bytecode,
                inputs,
                is_static,
                is_eof_init,
                spec_id,
                gas_limit,
            )))
        };

        Self { inner }
    }

    pub fn underlying_mut_ref(&mut self) -> &mut Interpreter<EthInterpreter<EXT, MG>> {
        match &mut self.inner {
            InternalInterpreter::Eth(interpreter) => interpreter,
            _ => panic!("Underlying interpreter is not Eth"),
        }
    }
}

impl<CTX, EXT: Default, MG: MemoryGetter> ArbInterpreter<CTX, EXT, MG>
where
    CTX: Host + BlockGetter + CfgGetter + Send,
    for<'a> CTX: 'a,
{
    /// Executes the interpreter until it returns or stops.
    pub fn run<FN>(
        &mut self,
        instruction_table: &[FN; 256],
        host: Arc<Mutex<CTX>>,
        cb: FrameCreateFunc<CTX>,
    ) -> InterpreterAction
    where
        FN: CustomInstruction<Wire = EthInterpreter<EXT, MG>, Host = CTX>,
        FN::Wire: InterpreterTypes,
    {
        self.inner.run(instruction_table, host, cb)
    }
}

impl<CTX, EXT: Default, MG: MemoryGetter> InternalInterpreter<CTX, EXT, MG>
where
    CTX: Host + BlockGetter + CfgGetter + Send,
    for<'a> CTX: 'a,
{
    fn run<FN>(
        &mut self,
        instruction_table: &[FN; 256],
        host: Arc<Mutex<CTX>>,
        cb: FrameCreateFunc<CTX>,
    ) -> InterpreterAction
    where
        FN: CustomInstruction<Wire = EthInterpreter<EXT, MG>, Host = CTX>,
        FN::Wire: InterpreterTypes,
    {
        match self {
            Self::Arb(interpreter) => interpreter.run(host, cb),
            Self::Eth(interpreter) => interpreter.run(instruction_table, &mut host.lock().unwrap()),
        }
    }
}
