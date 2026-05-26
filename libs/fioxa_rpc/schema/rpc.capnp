@0xaf2e99c022b03d53;

struct Call {
    interfaceId @0 :UInt64;

    methodId @1 :UInt16;

    payload @2 :AnyPointer;
}

struct Return {
    union {
        results @0 :AnyPointer;
        error @1 :Error;
    }
}

struct HandleIndex {
    index @0 :UInt8;
}

struct Error {
    reason @0 :Text;
    type @1 :Type;

    enum Type {
        failed @0;
        overloaded @1;
        disconnected @2;
        unimplemented @3;
    }
}

