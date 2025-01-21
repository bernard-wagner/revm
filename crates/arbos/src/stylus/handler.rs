use std::{cmp::min, mem};

use arbutil::evm::{
    api::{EvmApiMethod, Gas, VecReader},
    req::RequestHandler,
};
use revm::{
    context::Cfg,
    handler::FrameResult,
    interpreter::{
        self,
        gas::{self, sload_cost, sstore_cost},
        CreateInputs, CreateScheme, Host,
    },
};
use revm::{
    interpreter::{CallInputs, FrameInput},
    primitives::{Address, Log},
};

use super::revm_types;

pub struct StylusHandler<CTX: 'static> {
    pub address: Address,
    pub api: &'static mut CTX,
    pub new_frame_cb: FrameCreateFunc<CTX>,
    pub is_static: bool,
}

unsafe impl<CTX> Send for StylusHandler<CTX> {}

pub type FrameCreateFunc<CTX> = Box<dyn FnMut(&mut CTX, FrameInput) -> FrameResult>;

impl<CTX: Host + Send + 'static> StylusHandler<CTX> {
    pub fn new(
        context: &mut CTX,
        address: Address,
        cb: FrameCreateFunc<CTX>,
        is_static: bool,
    ) -> Self {
        let unsafe_context: &'static mut CTX = unsafe { mem::transmute(context) };

        Self {
            address,
            api: unsafe_context,
            new_frame_cb: cb,
            is_static,
        }
    }

    // Computes the cost of touching an account in wasm
    // Note: the code here is adapted from gasEip2929AccountCheck with the most recent parameters as of The Merge
    fn wasm_account_touch(&self, is_cold: bool, with_code: bool) -> u64 {
        let mut cost: u64 = 0;
        if with_code {
            cost = self.api.cfg().max_code_size() as u64 / 24576 * 700;
        }

        cost + gas::warm_cold_cost(is_cold)
    }
}

impl<CTX> RequestHandler<VecReader> for StylusHandler<CTX>
where
    CTX: Host + Send + 'static,
{
    fn request(
        &mut self,
        req_type: arbutil::evm::api::EvmApiMethod,
        req_data: impl AsRef<[u8]>,
    ) -> (Vec<u8>, VecReader, arbutil::evm::api::Gas) {
        let spec = self.api.cfg().spec().into();

        let mut data = req_data.as_ref().to_vec();
        match req_type {
            EvmApiMethod::GetBytes32 => {
                let slot = revm_types::take_u256(&mut data);
                if let Some(result) = self.api.sload(self.address, slot) {
                    let gas_cost = sload_cost(self.api.cfg().spec().into(), result.is_cold);
                    (
                        result.to_be_bytes_vec(),
                        VecReader::new(vec![]),
                        Gas(gas_cost),
                    )
                } else {
                    (vec![], VecReader::new(vec![]), Gas(0))
                }
            }
            EvmApiMethod::SetTrieSlots => {
                let gas_left = revm_types::take_u64(&mut data);

                if self.is_static {
                    return (
                        Status::WriteProtection.into(),
                        VecReader::new(vec![]),
                        Gas(gas_left),
                    );
                }

                let mut total_cost = 0;
                while !data.is_empty() {
                    let key = revm_types::take_u256(&mut data);
                    let value = revm_types::take_u256(&mut data);

                    let result = self.api.sstore(self.address, key, value);
                    if let Some(result) = result {
                        total_cost +=
                            sstore_cost(self.api.cfg().spec().into(), &result.data, result.is_cold);
                        if gas_left < total_cost {
                            return (
                                Status::OutOfGas.into(),
                                VecReader::new(vec![]),
                                Gas(gas_left),
                            );
                        }
                    } else {
                        return (
                            Status::Failure.into(),
                            VecReader::new(vec![]),
                            Gas(gas_left),
                        );
                    }
                }

                (
                    Status::Success.into(),
                    VecReader::new(vec![]),
                    Gas(gas_left - total_cost),
                )
            }
            EvmApiMethod::GetTransientBytes32 => {
                let slot = revm_types::take_u256(&mut data);
                let result = self.api.tload(self.address, slot);
                (result.to_be_bytes_vec(), VecReader::new(vec![]), Gas(0))
            }
            EvmApiMethod::SetTransientBytes32 => {
                if self.is_static {
                    return (
                        Status::WriteProtection.into(),
                        VecReader::new(vec![]),
                        Gas(0),
                    );
                }

                let key = revm_types::take_u256(&mut data);
                let value = revm_types::take_u256(&mut data);
                self.api.tstore(self.address, key, value);
                (Status::Success.into(), VecReader::new(vec![]), Gas(0))
            }
            EvmApiMethod::ContractCall | EvmApiMethod::DelegateCall | EvmApiMethod::StaticCall => {
                let address = revm_types::take_address(&mut data);
                let value = revm_types::take_u256(&mut data);
                let gas_left = revm_types::take_u64(&mut data);
                let gas_limit = revm_types::take_u64(&mut data);
                let calldata = revm_types::take_rest(&mut data);

                if self.is_static && !value.is_zero() {
                    return (
                        Status::WriteProtection.into(),
                        VecReader::new(vec![]),
                        Gas(gas_left),
                    );
                }

                let call_cost = if let Some(account_load) = self.api.load_account_delegated(address)
                {
                    gas::call_cost(spec, !value.is_zero(), account_load)
                } else {
                    return (
                        Status::Failure.into(),
                        VecReader::new(vec![]),
                        Gas(gas_left),
                    );
                };

                let gas_limit =
                    if spec.is_enabled_in(revm::specification::hardfork::SpecId::TANGERINE) {
                        // Take l64 part of gas_limit
                        min(gas_left - gas_left / 64, gas_limit)
                    } else {
                        gas_limit
                    };

                let res = (self.new_frame_cb)(
                    self.api,
                    FrameInput::Call(Box::new(CallInputs {
                        input: calldata,
                        return_memory_offset: 0..0,
                        gas_limit,
                        bytecode_address: address,
                        target_address: address,
                        caller: self.address,
                        value: revm::interpreter::CallValue::Transfer(value),
                        scheme: revm::interpreter::CallScheme::Call,
                        is_static: self.is_static,
                        is_eof: false,
                    })),
                );

                let mut gas = gas::Gas::new(gas_limit + call_cost);
                gas.spend_all();

                if let FrameResult::Call(result) = res {
                    gas.erase_cost(result.gas().remaining());

                    (
                        Status::Success.into(),
                        VecReader::new(result.result.output.to_vec()),
                        Gas(gas.spent()),
                    )
                } else {
                    (vec![], VecReader::new(vec![]), Gas(gas.spent()))
                }
            }
            EvmApiMethod::Create1 | EvmApiMethod::Create2 => {
                let is_create_2 = matches!(req_type, EvmApiMethod::Create2);
                let gas_remaining = revm_types::take_u64(&mut data);
                let value = revm_types::take_u256(&mut data);
                let salt = if is_create_2 {
                    Some(revm_types::take_u256(&mut data))
                } else {
                    None
                };
                let init_code = revm_types::take_rest(&mut data);

                if self.is_static {
                    return (
                        Status::WriteProtection.into(),
                        VecReader::new(vec![]),
                        Gas(0),
                    );
                }

                if is_create_2
                    && !spec.is_enabled_in(revm::specification::hardfork::SpecId::PETERSBURG)
                {
                    return (
                        Status::Failure.into(),
                        VecReader::new(vec![]),
                        Gas(gas_remaining),
                    );
                }

                let mut gas_cost = 0;

                let len = init_code.len();

                if len != 0 {
                    if spec.is_enabled_in(revm::specification::hardfork::SpecId::SHANGHAI) {
                        let max_initcode_size = self.api.cfg().max_code_size().saturating_mul(2);
                        if init_code.len() > max_initcode_size {
                            return (
                                Status::Failure.into(),
                                VecReader::new(vec![]),
                                Gas(gas_remaining),
                            );
                        }

                        gas_cost = gas::initcode_cost(init_code.len());
                    }

                    let num_word = interpreter::num_words(len);
                    gas_cost += gas::memory_gas(num_word);
                }

                let scheme = if is_create_2 {
                    if let Some(check_cost) =
                        gas::create2_cost(len).and_then(|cost| gas_cost.checked_add(cost))
                    {
                        gas_cost = check_cost;
                    } else {
                        return (
                            Status::Failure.into(),
                            VecReader::new(vec![]),
                            Gas(gas_remaining),
                        );
                    };
                    CreateScheme::Create2 {
                        salt: salt.unwrap(),
                    }
                } else {
                    gas_cost += gas::CREATE;
                    CreateScheme::Create
                };

                if gas_remaining < gas_cost {
                    return (
                        Status::OutOfGas.into(),
                        VecReader::new(vec![]),
                        Gas(gas_remaining),
                    );
                }

                let mut gas_limit = gas_remaining - gas_cost;

                if spec.is_enabled_in(revm::specification::hardfork::SpecId::TANGERINE) {
                    // Take remaining gas and deduce l64 part of it.
                    gas_limit -= gas_limit / 64;
                }

                let result = (self.new_frame_cb)(
                    self.api,
                    FrameInput::Create(Box::new(CreateInputs {
                        caller: self.address,
                        scheme,
                        value,
                        init_code,
                        gas_limit,
                    })),
                );

                gas_cost += result.gas().spent();

                if gas_remaining < gas_cost {
                    return (
                        Status::OutOfGas.into(),
                        VecReader::new(vec![]),
                        Gas(gas_remaining),
                    );
                }

                let output = result.output();

                match result {
                    FrameResult::Create(create_outcome) => match create_outcome.result.result {
                        interpreter::InstructionResult::Return => {
                            let address = create_outcome.address.unwrap_or_default();
                            // prefix address with 0x01
                            let mut address_bytes = vec![1];
                            address_bytes.extend(address.to_vec());

                            (
                                address_bytes,
                                VecReader::new(output.data().to_vec()),
                                Gas(gas_cost),
                            )
                        }
                        interpreter::InstructionResult::Revert => {
                            let address = Address::ZERO;
                            // prefix address with 0x01
                            let mut address_bytes = vec![1];
                            address_bytes.extend(address.to_vec());

                            (
                                address_bytes,
                                VecReader::new(output.data().to_vec()),
                                Gas(gas_cost),
                            )
                        }
                        _ => (
                            Status::Failure.into(),
                            VecReader::new(vec![]),
                            Gas(gas_remaining),
                        ),
                    },
                    _ => (
                        Status::Failure.into(),
                        VecReader::new(vec![]),
                        Gas(gas_remaining),
                    ),
                }
            }
            EvmApiMethod::EmitLog => {
                let mut topics = Vec::new();
                let topic_count = revm_types::take_u32(&mut data);
                for _ in 0..topic_count {
                    topics.push(revm_types::take_bytes32(&mut data));
                }
                let data = revm_types::take_rest(&mut data);

                let log = Log::new_unchecked(self.address, topics, data);

                self.api.log(log);

                (vec![], VecReader::new(vec![]), Gas(0))
            }
            EvmApiMethod::AccountBalance => {
                let address = revm_types::take_address(&mut data);
                let balance = self.api.balance(address).unwrap();
                let gas_cost = self.wasm_account_touch(balance.is_cold, false);
                (
                    balance.to_be_bytes_vec(),
                    VecReader::new(vec![]),
                    Gas(gas_cost),
                )
            }
            EvmApiMethod::AccountCode => {
                let address = revm_types::take_address(&mut data);
                let code = self.api.code(address).unwrap();
                let gas_cost = self.wasm_account_touch(code.is_cold, true);
                (vec![], VecReader::new(code.to_vec()), Gas(gas_cost))
            }
            EvmApiMethod::AccountCodeHash => {
                let address = revm_types::take_address(&mut data);
                let code_hash = self.api.code_hash(address).unwrap();
                let gas_cost = self.wasm_account_touch(code_hash.is_cold, false);
                (code_hash.to_vec(), VecReader::new(vec![]), Gas(gas_cost))
            }
            EvmApiMethod::AddPages => {
                let _count = revm_types::take_u16(&mut data);
                // TODO: for now it is always free
                (Status::Success.into(), VecReader::new(vec![]), Gas(0))
            }
            EvmApiMethod::CaptureHostIO => (Status::Success.into(), VecReader::new(vec![]), Gas(0)),
        }
    }
}

enum Status {
    Success,
    Failure,
    OutOfGas,
    WriteProtection,
}

impl From<Status> for Vec<u8> {
    fn from(status: Status) -> Vec<u8> {
        match status {
            Status::Success => vec![0],
            Status::Failure => vec![1],
            Status::OutOfGas => vec![2],
            Status::WriteProtection => vec![3],
        }
    }
}
