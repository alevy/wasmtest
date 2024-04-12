use wasmtime::*;
use std::collections::HashMap;
use lambda_http::{run, service_fn, tracing, Body, Error, Request, Response};

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing::init_default_subscriber();

    run(service_fn(function_handler)).await
}

trait Datastore {
    async fn put_item(&mut self, key: Vec<u8>, value: Vec<u8>);
    async fn get_item(&mut self, key: &Vec<u8>) -> Option<Vec<u8>>;
}

impl Datastore for HashMap<Vec<u8>, Vec<u8>> {
    async fn put_item(&mut self, key: Vec<u8>, value: Vec<u8>) {
	self.insert(key, value);
    }

    async fn get_item(&mut self, key: &Vec<u8>) -> Option<Vec<u8>> {
	self.get(key).map(Clone::clone)
    }
}

struct DynamoDBDatastore {
    client: aws_sdk_dynamodb::Client,
    table_name: String,
}

impl Datastore for DynamoDBDatastore {
    async fn put_item(&mut self, key: Vec<u8>, value: Vec<u8>) {
	use aws_sdk_dynamodb::{types::AttributeValue, primitives::Blob};
	let key = String::from_utf8_lossy(&key).to_string();
	self.client.put_item().table_name(self.table_name.clone()).item(key, AttributeValue::B(Blob::new(value))).send().await.expect("put_item");
    }

    async fn get_item(&mut self, key: &Vec<u8>) -> Option<Vec<u8>> {
	use aws_sdk_dynamodb::{types::AttributeValue, primitives::Blob};
	let key = String::from_utf8_lossy(&key).to_string();
	let result = self.client.get_item().table_name(self.table_name.clone())
	    .key(key.clone(), AttributeValue::B(Blob::new(b""))).send().await.expect("get_item");
	match result.item.and_then(|i| i.get(&key).map(Clone::clone)) {
	    Some(r) => match r.as_b().ok() {
		Some(b) => Some(b.clone().into_inner()),
		None => None,
	    },
	    None => None,
	}
    }
}

#[derive(Debug)]
struct MyState<D: Datastore> {
    database: D,
}

async fn function_handler(event: Request) -> Result<Response<Body>, Error> {
    let body = &event.body();

    // First the wasm module needs to be compiled. This is done with a global
    // "compilation environment" within an `Engine`. Note that engines can be
    // further configured through `Config` if desired instead of using the
    // default like this is here.
    let mut config = Config::new();
    config.async_support(true);
    let engine = Engine::new(&config)?;
    let module = Module::from_file(&engine, "../wasmtest/target/wasm32-unknown-unknown/release/wasmtest.wasm")?;

    // After a module is compiled we create a `Store` which will contain
    // instantiated modules and other items like host functions. A Store
    // contains an arbitrary piece of host information, and we use `MyState`
    // here.

    let mut state = MyState {
	database: HashMap::new(),
    };

    state.database.insert(b"foo".into(), b"bar".into());

    let mut store = Store::new(
        &engine,
	state,
    );

    // Our wasm module we'll be instantiating requires one imported function.
    // the function takes no parameters and returns no results. We create a host
    // implementation of that function here, and the `caller` parameter here is
    // used to get access to our original `MyState` value.
    let write_key_func = Func::wrap4_async(&mut store, |mut caller: Caller<'_, _>, key_base: u32, key_len: u32, value_base: u32, value_len: u32| {
	Box::new(async move {
	    let memory = caller.get_export("memory").and_then(|m| m.into_memory()).unwrap();
	    let mut key = Vec::new();
	    key.resize(key_len as usize, 0);
	    memory.read(caller.as_context_mut(), key_base as usize, key.as_mut_slice()).unwrap();

	    let mut value = Vec::new();
	    value.resize(value_len as usize, 0);
	    memory.read(caller.as_context_mut(), value_base as usize, value.as_mut_slice()).unwrap();

	    let state = caller.data_mut();

	    println!("writing {:?} {:?}", String::from_utf8(key.clone()), String::from_utf8(value.clone()));
	    state.database.insert(key, value);
	})
    });
    let read_key_func = Func::wrap3_async(&mut store, |mut caller: Caller<'_, _>, result_base: u32, key_base: u32, key_len: u32| {
	Box::new(async move {
	    let memory = caller.get_export("memory").and_then(|m| m.into_memory()).unwrap();
	    let mut key = Vec::new();
	    key.resize(key_len as usize, 0);
	    memory.read(caller.as_context_mut(), key_base as usize, key.as_mut_slice()).unwrap();

	    let state = caller.data();
	    let result = state.database.get(&key).unwrap_or(&Vec::new()).clone();

	    let result_offset = memory.data_size(caller.as_context()) - result.len();
	    memory.write(caller.as_context_mut(), result_offset, result.as_slice()).unwrap();
	    memory.write(caller.as_context_mut(), result_base as usize, &((result_offset as u32).to_le_bytes())).unwrap();
	    memory.write(caller.as_context_mut(), result_base as usize + 4, &((result.len() as u32).to_le_bytes())).unwrap();

	    println!("reading {:?} {:?}", String::from_utf8(key.clone()), String::from_utf8(result));
	})
    });

    // Once we've got that all set up we can then move to the instantiation
    // phase, pairing together a compiled module as well as a set of imports.
    // Note that this is where the wasm `start` function, if any, would run.
    let imports = [write_key_func.into(), read_key_func.into()];//, input_body.into(), response_body.into()];
    let instance = Instance::new_async(&mut store, &module, &imports).await?;

    // Next we poke around a bit to extract the `entry` function from the module.
    let memory = instance.get_memory(&mut store, "memory").unwrap();
    memory.write(&mut store, 8, body)?;
    let run = instance.get_typed_func::<(i32, i32, i32), ()>(&mut store, "entry")?;

    // And last but not least we can call it!
    run.call_async(&mut store, (0, 8, body.len() as i32)).await?;

    let mut result_base_bytes = [0; 4];
    let mut result_len_bytes = [0; 4];
    memory.read(&store, 0, &mut result_base_bytes)?;
    memory.read(&store, 4, &mut result_len_bytes)?;

    let result_base = i32::from_le_bytes(result_base_bytes) as usize;
    let result_len = i32::from_le_bytes(result_len_bytes) as usize;
    let result_slice = &memory.data(&store)[result_base..][..result_len];

    // Return something that implements IntoResponse.
    // It will be serialized to the right response event automatically by the runtime
    let resp = Response::builder()
        .status(200)
        .header("content-type", "text/html")
        .body(result_slice.into())
        .map_err(Box::new)?;
    Ok(resp)
}
