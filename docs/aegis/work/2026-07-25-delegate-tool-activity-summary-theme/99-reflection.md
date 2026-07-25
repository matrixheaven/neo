# Delegate Tool Activity Summary And Theme - Reflection

The shared child-activity renderer now owns semantic tool and file spans for
all Delegate-family consumers. Collapsed DelegateSwarm rows retain their
one-line layout while showing one deterministic risk-first file and truthful
aggregate totals. The old plain formatter and re-export were removed.

Spec and code-quality review found and closed three compatibility gaps:
Pending totals, legacy non-Edit/Write truncation, and neutral styling for
ongoing tools. Focused tests, scoped formatting/diff checks, and the Neo binary
check pass. No core/runtime/schema/theme-schema or card-layout boundary moved.

Method Pack output does not grant completion authority.
