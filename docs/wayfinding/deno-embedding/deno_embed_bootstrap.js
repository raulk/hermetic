globalThis.hermetic = {
  rpc(request) {
    return JSON.parse(Deno.core.ops.op_hermetic_rpc(JSON.stringify(request)));
  },
};
