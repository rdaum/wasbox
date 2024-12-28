
itoa.wasm:	file format wasm 0x1

Code Disassembly:
q
00004b func[1] <itoa>:
 00004c: 05 7f                      | local[1..5] type=i32
 00004e: 20 00                      | local.get 0
 000050: 41 0a                      | i32.const 10
 000052: 48                         | i32.lt_s
 000053: 04 40                      | if
 000055: 41 01                      |   i32.const 1
 000057: 21 02                      |   local.set 2
 000059: 05                         | else
 00005a: 41 00                      |   i32.const 0
 00005c: 21 02                      |   local.set 2
 00005e: 20 00                      |   local.get 0
 000060: 21 01                      |   local.set 1
 000062: 03 40                      |   loop
 000064: 02 40                      |     block
 000066: 20 01                      |       local.get 1
 000068: 45                         |       i32.eqz
 000069: 0d 00                      |       br_if 0
 00006b: 20 01                      |       local.get 1
 00006d: 41 0a                      |       i32.const 10
 00006f: 6e                         |       i32.div_u
 000070: 21 01                      |       local.set 1
 000072: 20 02                      |       local.get 2
 000074: 41 01                      |       i32.const 1
 000076: 6a                         |       i32.add
 000077: 21 02                      |       local.set 2
 000079: 0c 01                      |       br 1
 00007b: 0b                         |     end
 00007c: 0b                         |   end
 00007d: 0b                         | end
 00007e: 23 00                      | global.get 0
 000080: 20 02                      | local.get 2
 000082: 6a                         | i32.add
 000083: 41 01                      | i32.const 1
 000085: 6b                         | i32.sub
 000086: 21 03                      | local.set 3
 000088: 03 40                      | loop
 00008a: 02 40                      |   block
 00008c: 20 00                      |     local.get 0
 00008e: 41 0a                      |     i32.const 10
 000090: 70                         |     i32.rem_u
 000091: 21 04                      |     local.set 4
 000093: 20 04                      |     local.get 4
 000095: 2d 00 c0 3e                |     i32.load8_u 0 8000
 000099: 21 05                      |     local.set 5
 00009b: 20 03                      |     local.get 3
 00009d: 20 05                      |     local.get 5
 00009f: 3a 00 00                   |     i32.store8 0 0
 0000a2: 20 00                      |     local.get 0
 0000a4: 41 0a                      |     i32.const 10
 0000a6: 6e                         |     i32.div_u
 0000a7: 21 00                      |     local.set 0
 0000a9: 20 03                      |     local.get 3
 0000ab: 23 00                      |     global.get 0
 0000ad: 46                         |     i32.eq
 0000ae: 0d 00                      |     br_if 0
 0000b0: 20 03                      |     local.get 3
 0000b2: 41 01                      |     i32.const 1
 0000b4: 6b                         |     i32.sub
 0000b5: 21 03                      |     local.set 3
 0000b7: 0c 01                      |     br 1
 0000b9: 0b                         |   end
 0000ba: 0b                         | end
 0000bb: 23 00                      | global.get 0
 0000bd: 20 02                      | local.get 2
 0000bf: 0b                         | end
