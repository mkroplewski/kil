# Routing KIL projects

Read this reference only for routing work. `kil route` uses the KiCadRoutingTools version bundled with the installed KIL release.

## Preconditions

The root `build.routing` profile selects the router independently of PCB intent. Query `kil schema` for the current structure. A minimal policy selects the bundled engine and defaults to all nets:

```json
{
  "build": {
    "routing": {
      "engine": "kicad-routing-tools",
      "nets": ["*"]
    }
  }
}
```

Run `kil lock FILE` once to accept resolved library contents. For an existing project with a current cache, run `kil check` before routing and fix structural errors first. If its cache is missing or stale, run `kil route FILE` first after the structural preflight; omit net selection to reroute all nets when the cache is stale. Then run `kil check` and `kil build`. For a new project that already declares routing but has no cache, run `kil route` directly after the preflight in `SKILL.md`; a preceding check would only report the expected missing cache. The route command validates before invoking the router.

After a router run, group related placement or rule fixes into one edit before trying again. Do not repeat unchanged schema, library, or whole-project inspection commands. ERC or existing DRC findings may remain if they do not prevent the requested routing work, but record them so new violations are distinguishable.

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

The router works on a temporary KiCad board. `kil` converts accepted copper into `*.kil.routes.json` and fingerprints routing inputs. Never patch this cache manually. Selected-net routing seeds from a valid cache and preserves other nets and their original lock flags. After a source change makes a cache stale, reroute all nets; a targeted pass must not carry stale copper forward.

After routing, run `kil check` or `kil build`. A stale cache after a placement, net, rule, or seed-route change is an error and must be regenerated.

Treat incomplete pad pairs, open nets, or a successful router process that emits no copper as routing failures. Do not replace a prior valid cache with such a result.

The source format remains two-layer and does not express blind/buried vias or native differential-pair constraints. Review routed output in KiCad and report remaining DRC findings.
