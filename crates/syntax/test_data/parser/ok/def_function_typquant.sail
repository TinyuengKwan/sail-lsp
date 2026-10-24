default Order dec

function mmio_read forall 'n, 0 < 'n <= 8 . (width : int('n)) -> bits(8 * 'n) =
  clint_load(width)

function tlb_hash forall 'v, is_sv_mode('v) . (mode : int('v)) -> bits(4) =
  0x0
