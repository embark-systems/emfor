"""gRPC servers for the emfor-check live tests (see inside.sh).

127.0.0.1:50051  plaintext, grpc.health.v1.Health: service "" is SERVING,
                 "down" is NOT_SERVING, anything else is NOT_FOUND
127.0.0.1:50052  the same over TLS (argv[1] certificate, argv[2] key)
127.0.0.1:50053  plaintext without a health service (UNIMPLEMENTED)

The health service answers raw protobuf bytes through a generic handler, so
no generated code is needed.
"""

import sys
import threading
from concurrent import futures

import grpc


def requested_service(request: bytes) -> bytes:
    """Field 1 of a HealthCheckRequest (lengths below 128 only)."""
    if len(request) >= 2 and request[0] == 0x0A:
        return request[2 : 2 + request[1]]
    return b""


def check(request, context):
    service = requested_service(request)
    if service == b"":
        return b"\x08\x01"
    if service == b"down":
        return b"\x08\x02"
    context.abort(grpc.StatusCode.NOT_FOUND, "unknown service")


def health_handler():
    return grpc.method_handlers_generic_handler(
        "grpc.health.v1.Health",
        {"Check": grpc.unary_unary_rpc_method_handler(check)},
    )


def main():
    with open(sys.argv[1], "rb") as f:
        cert = f.read()
    with open(sys.argv[2], "rb") as f:
        key = f.read()
    servers = []
    for port, health, tls in ((50051, True, False), (50052, True, True), (50053, False, False)):
        server = grpc.server(futures.ThreadPoolExecutor(max_workers=4))
        if health:
            server.add_generic_rpc_handlers((health_handler(),))
        address = f"127.0.0.1:{port}"
        if tls:
            server.add_secure_port(address, grpc.ssl_server_credentials([(key, cert)]))
        else:
            server.add_insecure_port(address)
        server.start()
        servers.append(server)
    threading.Event().wait()


if __name__ == "__main__":
    main()
