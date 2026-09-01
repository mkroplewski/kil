# Routing KIL projects

Read this reference only for routing work. `kil route` uses the KiCadRoutingTools version bundled with the installed KIL release.

## Preconditions

The root PCB needs a routing policy. Query `kil schema` for the current structure. A minimal policy selects the bundled engine and defaults to all nets:

```json
{
  "routing": {
    "engine": "kicad-routing-tools",
    "nets": ["*"]
  }
}
```

Run `kil check` before routing. Fix structural errors first. ERC or existing DRC findings may remain if they do not prevent the requested routing work, but record them so new violations are distinguishable.

## Route narrowly

For a large board, prefer one block or a small net group:

```console
kil route board.kil.json --block controller
kil route board.kil.json --net "/USB_*"
```

Use an unrestricted route only when the user asks for it or the board is small enough to review as one operation:

```console
kil route board.kil.json
```

`--block` selects every mapped net owned by the block. A shared net such as global `GND` still spans the whole board. Lock finished copper before incremental routing when exposing a shared net would let the router alter accepted geometry.

## Route cache

The router works on a temporary KiCad board. `kil` converts accepted copper into `*.kil.routes.json` and fingerprints routing inputs. Never patch this cache manually.

After routing, run `kil check` or `kil build`. A stale cache after a placement, net, rule, or seed-route change is an error and must be regenerated.

Treat incomplete pad pairs, open nets, or a successful router process that emits no copper as routing failures. Do not replace a prior valid cache with such a result.

KiCadRoutingTools has no push-and-shove, blind or buried vias, coarse global-routing pass, or region-specific design rules. Review routed output in KiCad for dense boards and report any remaining DRC findings.
