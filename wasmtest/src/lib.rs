pub mod datastore {
    use super::WasmBytes;
    extern {
        fn write_key(key: WasmBytes, body: WasmBytes);
        fn read_key(key: WasmBytes) -> WasmBytes;
    }

    pub fn write(key: &[u8], body: &[u8]) {
        unsafe {
            write_key(WasmBytes::from_slice(key), WasmBytes::from_slice(body))
        }
    }

    pub fn read<F, R>(key: &[u8], mut f: F) -> R where F: (FnMut(&[u8]) -> R) {
        unsafe {
            let result = read_key(WasmBytes::from_slice(key));
	    f(result.as_slice())
	}
    }
}

#[repr(C)]
pub struct WasmBytes {
    base: *const u8,
    len: usize,
}

impl WasmBytes {
    pub fn from_slice(s: &[u8]) -> Self {
        WasmBytes {
            base: s.as_ptr(),
            len: s.len()
        }
    }

    pub fn as_slice(&self) -> &[u8] {
	unsafe {
	    std::slice::from_raw_parts(self.base, self.len)
	}
    }
}


#[no_mangle]
pub fn entry(result: &mut WasmBytes, body: WasmBytes) {
    let body = body.as_slice();
    datastore::write(body, b"world");
    let res: Vec<u8> = datastore::read(b"foo", |value| {
	datastore::write(b"world", value);
	value.into()
    });
    *result = WasmBytes::from_slice(&res);
    std::mem::forget(res);
}
