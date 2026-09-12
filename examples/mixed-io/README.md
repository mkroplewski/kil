# Mixed I/O reference

This board exercises KIL's compiler on a larger design. It has an STM32F103C8Tx, eight protected analog inputs, sixteen digital I/O channels with indicators, an SWD connector, and an external 3.3 V supply connector. There are 203 physical parts and 24 child schematic sheets.

The format stays small through repeated modules. `blocks/analog.kil.json` defines one analog channel; `blocks/digital.kil.json` defines one digital channel. The root connects their `signal`, `vcc`, and `gnd` ports and positions each instance. The same module supplies its schematic page and PCB placement. Private nets and page identities are qualified automatically.

The four-layer board uses `In1.Cu` for a ground plane and `In2.Cu` for a 3.3 V plane. Mounting holes and a reserved edge region exercise mechanical constraints. Two power-source declarations identify the external supply and return. No separate power-flag parts are authored.

Generate a local library lock and route with KiCad 10 and the version of KiCadRoutingTools pinned in `KRT_VERSION`:

```sh
kil lock examples/mixed-io/project.kil.json
kil route examples/mixed-io/project.kil.json --krt /path/to/KiCadRoutingTools
kil build examples/mixed-io/project.kil.json
```

This is a compiler reference, not a fabrication-qualified circuit. The MCU uses its internal oscillator and an external regulated supply. Analog accuracy, protection ratings, signal integrity, power integrity, EMC, and assembly have not been qualified. Routing diagnostics must be reviewed before treating a generated board as complete.
