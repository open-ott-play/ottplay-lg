//! Strict read-only connection handler. The dedicated listener never runs ordinary setup.
use super::super::wire::{self, Descriptor, ErrorCode, Request, Response};
use super::{Backend, Rpc};
use std::io::{self, Read, Write};

/// Caller already authenticated the Unix peer on the dedicated inspection endpoint.
/// There are no filesystem diagnostics, capability probes, or write-capable commands here.
pub fn serve(
    stream: &mut (impl Read + Write),
    descriptor: &Descriptor,
    backend: &mut Backend<impl Rpc>,
    deadline: std::time::Instant,
) -> io::Result<()> {
    wire::inspection_io::check(deadline)?;
    let hello = wire::read_frame::<Request>(stream);
    wire::inspection_io::check(deadline)?;
    if !matches!(hello, Ok(Request::InspectHello { protocol, nonce })
        if protocol == wire::PROTOCOL && nonce == descriptor.nonce)
    {
        return wire::write_frame(
            stream,
            &Response::Error {
                code: ErrorCode::Protocol,
            },
        );
    }
    wire::write_frame(
        stream,
        &Response::InspectHello {
            protocol: wire::PROTOCOL,
            nonce: descriptor.nonce.clone(),
            helper_generation: descriptor.helper_generation.clone(),
        },
    )?;
    let response = match wire::read_frame::<Request>(stream) {
        Ok(request) => {
            wire::inspection_io::check(deadline)?;
            backend.inspect(request)
        },
        Err(_) => Response::Error {
            code: ErrorCode::Protocol,
        },
    };
    wire::inspection_io::check(deadline)?;
    wire::write_frame(stream, &response)
}

#[cfg(test)]
#[path = "inspection_tests.rs"]
mod tests;
