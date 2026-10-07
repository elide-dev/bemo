# Shared-view ownership qualification

[CI run](https://github.com/elide-dev/bemo/actions/runs/37581322288) compares pre-stack plain Arc at
`499a6f3dc4037fa8b7e264fe4928e7c3305b665a` with the current inline leases at
`a87e7abcec4ee5b317a9212b7839a1d04fc0702d` on one AMD EPYC 9V45 Linux runner.
Both release benchmark binaries were built before timing. Three rounds alternate
version order; each case has a one-second warmup, two-second measurement, and
30 Criterion samples. [Raw measurements and CPU/affinity](data/lease-qualification.json)
retain all estimates. The table shows medians and ranges across the three round
estimates; those ranges are not confidence intervals.

| Operation | Before ns (range) | Current ns (range) | Change |
| --- | ---: | ---: | ---: |
| buffer/retain-drop/4096 | 9.827 (9.530–9.841) | 9.850 (9.765–9.892) | +0.2% |
| buffer/slice-drop/4096 | 10.288 (10.146–10.354) | 10.232 (10.230–10.262) | -0.5% |
| buffer/exclusive-freeze-recover | 6.335 (6.218–6.399) | 6.349 (6.294–6.379) | +0.2% |

The measured changes are about one percent or less for retain/drop and
slice/drop, and a fraction of a nanosecond for exclusive recovery. This does
not reproduce a material shared-view regression. Keep the current atomic Arc
ownership and inline lease representation. An intrusive reference count would
add ownership risk without evidence of a useful throughput gain here.

These are uncontended microbenchmarks. They do not qualify concurrent final
drop, allocation reuse, instruction/cache counters, or the full transport.
The prior CodSpeed comparison used different host/cache models; its modeled
memory penalties cannot be read as same-host wall-clock regression.
