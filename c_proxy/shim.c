#include <rpcproxy.h>
const IID IID_IInspectable = {0xAF86E2E0,0xB12D,0x4C6A,{0x9C,0x5A,0xD7,0xAA,0x65,0x10,0x1E,0x90}};
extern CStdPSFactoryBuffer gPFactory;
ULONG STDMETHODCALLTYPE CStdStubBuffer2_Release(IRpcStubBuffer *This) {
    return NdrCStdStubBuffer2_Release(This, (IPSFactoryBuffer *)&gPFactory);
}
