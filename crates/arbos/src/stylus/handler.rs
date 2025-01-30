use std::{cell::RefCell, cmp::min, rc::Rc};

use arbutil::evm::{
    api::{EvmApiMethod, Gas, VecReader},
    req::RequestHandler,
};
use revm::{
    context::Cfg,
    context_interface::CfgGetter,
    handler::FrameResult,
    interpreter::{
        self,
        gas::{self, sload_cost, sstore_cost},
        CallInputs, CreateInputs, CreateScheme, FrameInput, Host,
    },
    primitives::{Address, Log},
};

use super::revm_types;

pub struct StylusHandler<CTX> {
    pub address: Address,
    pub api: Rc<RefCell<CTX>>,
    pub new_frame_cb: FrameCreateFunc<CTX>,
    pub is_static: bool,
}

unsafe impl<CTX> Send for StylusHandler<CTX> {}

pub type FrameCreateFunc<CTX> = Box<dyn FnMut(Rc<RefCell<CTX>>, FrameInput) -> FrameResult>;

impl<CTX: CfgGetter> StylusHandler<CTX> {
    pub fn new(
        context: Rc<RefCell<CTX>>,
        address: Address,
        cb: FrameCreateFunc<CTX>,
        is_static: bool,
    ) -> Self {
        Self {
            address,
            api: context,
            new_frame_cb: cb,
            is_static,
        }
    }

    fn wasm_account_touch(&self, is_cold: bool, with_code: bool) -> u64 {
        let code_cost = if with_code {
            self.api.borrow_mut().cfg().max_code_size() as u64 / 24576 * 700
        } else {
            0
        };
        code_cost + gas::warm_cold_cost(is_cold)
    }
}

impl<CTX: Host + 'static> RequestHandler<VecReader> for StylusHandler<CTX> {
    fn request(
        &mut self,
        req_type: EvmApiMethod,
        req_data: impl AsRef<[u8]>,
    ) -> (Vec<u8>, VecReader, Gas) {
        let mut api = self.api.borrow_mut();
        let spec = api.cfg().spec().into();
        let mut data = req_data.as_ref().to_vec();

        match req_type {
            EvmApiMethod::GetBytes32 => {
                let slot = revm_types::take_u256(&mut data);
                if let Some(result) = api.sload(self.address, slot) {
                    let gas = sload_cost(spec, result.is_cold);
                    (result.to_be_bytes_vec(), VecReader::new(vec![]), Gas(gas))
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

                    if let Some(result) = api.sstore(self.address, key, value) {
                        total_cost += sstore_cost(spec, &result.data, result.is_cold);
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
                let result = api.tload(self.address, slot);
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
                api.tstore(self.address, key, value);
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

                let Some(account_load) = api.load_account_delegated(address) else {
                    return (
                        Status::Failure.into(),
                        VecReader::new(vec![]),
                        Gas(gas_left),
                    );
                };

                let call_cost = gas::call_cost(spec, !value.is_zero(), account_load);
                let gas_limit =
                    if spec.is_enabled_in(revm::specification::hardfork::SpecId::TANGERINE) {
                        min(gas_left - gas_left / 64, gas_limit)
                    } else {
                        gas_limit
                    };

                let res = (self.new_frame_cb)(
                    self.api.clone(),
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
                let salt = is_create_2.then(|| revm_types::take_u256(&mut data));
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
                        let max_initcode_size = api.cfg().max_code_size().saturating_mul(2);
                        if len > max_initcode_size {
                            return (
                                Status::Failure.into(),
                                VecReader::new(vec![]),
                                Gas(gas_remaining),
                            );
                        }
                        gas_cost = gas::initcode_cost(len);
                    }
                    gas_cost += gas::memory_gas(interpreter::num_words(len));
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
                    gas_limit -= gas_limit / 64;
                }

                let result = (self.new_frame_cb)(
                    self.api.clone(),
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
                    FrameResult::Create(create_outcome) => {
                        let mut address_bytes = vec![1];
                        match create_outcome.result.result {
                            interpreter::InstructionResult::Return => {
                                address_bytes
                                    .extend(create_outcome.address.unwrap_or_default().to_vec());
                            }
                            interpreter::InstructionResult::Revert => {
                                address_bytes.extend(Address::ZERO.to_vec());
                            }
                            _ => {
                                return (
                                    Status::Failure.into(),
                                    VecReader::new(vec![]),
                                    Gas(gas_remaining),
                                )
                            }
                        }
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
                }
            }

            EvmApiMethod::EmitLog => {
                let topic_count = revm_types::take_u32(&mut data);
                let mut topics = Vec::with_capacity(topic_count as usize);
                for _ in 0..topic_count {
                    topics.push(revm_types::take_bytes32(&mut data));
                }
                let data = revm_types::take_rest(&mut data);
                api.log(Log::new_unchecked(self.address, topics, data));
                (vec![], VecReader::new(vec![]), Gas(0))
            }

            EvmApiMethod::AccountBalance => {
                let address = revm_types::take_address(&mut data);
                let balance = api.balance(address).unwrap();
                let gas = self.wasm_account_touch(balance.is_cold, false);
                (balance.to_be_bytes_vec(), VecReader::new(vec![]), Gas(gas))
            }

            EvmApiMethod::AccountCode => {
                let address = revm_types::take_address(&mut data);
                let code = api.code(address).unwrap();
                let gas = self.wasm_account_touch(code.is_cold, true);
                (vec![], VecReader::new(code.to_vec()), Gas(gas))
            }

            EvmApiMethod::AccountCodeHash => {
                let address = revm_types::take_address(&mut data);
                let code_hash = api.code_hash(address).unwrap();
                let gas = self.wasm_account_touch(code_hash.is_cold, false);
                (code_hash.to_vec(), VecReader::new(vec![]), Gas(gas))
            }

            EvmApiMethod::AddPages => {
                let _count = revm_types::take_u16(&mut data);
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
