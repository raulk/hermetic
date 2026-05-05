globalThis.undercover = {
  rpc(request) {
    return JSON.parse(Deno.core.ops.op_undercover_rpc(JSON.stringify(request)));
  },
};
