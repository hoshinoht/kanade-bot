"""Pure validation functions for inventory regression tests and the checker."""

from __future__ import annotations

from collections import Counter
from collections.abc import Iterable, Mapping, Sequence
from typing import Any

DISPOSITIONS = {"Retain", "Import-only", "Remove", "Defer"}


def _duplicates(values: Iterable[str]) -> set[str]:
    counts = Counter(values)
    return {value for value, count in counts.items() if count > 1}


def validate_entrypoint_inventory(
    items: Sequence[Mapping[str, Any]], discovered: Iterable[str]
) -> list[str]:
    errors: list[str] = []
    ids = [str(item.get("id", "")) for item in items]
    duplicate_ids = _duplicates(ids)
    if duplicate_ids:
        errors.append(f"duplicate entrypoints: {sorted(duplicate_ids)}")
    expected = set(ids)
    found = set(discovered)
    if expected != found:
        errors.append(
            f"entrypoint set mismatch: discovered={sorted(found)}, expected={sorted(expected)}"
        )
    for item in items:
        if item.get("disposition") not in DISPOSITIONS:
            errors.append(f"invalid entrypoint disposition: {item.get('id')}")
        if not item.get("owner") or not item.get("source") or not item.get("tests"):
            errors.append(f"incomplete entrypoint evidence: {item.get('id')}")
    return errors


def validate_healthcheck_inventory(
    items: Sequence[Mapping[str, Any]],
    discovered_entrypoints: Iterable[str],
    discovered_configured: Mapping[str, str | None],
) -> list[str]:
    errors: list[str] = []
    ids = [str(item.get("id", "")) for item in items]
    expected = set(ids)
    configured = {command for command in discovered_configured.values() if command}
    if expected != configured:
        errors.append(
            "healthcheck set mismatch: "
            f"discovered={sorted(configured)}, expected={sorted(expected)}"
        )
    entrypoints = set(discovered_entrypoints)
    for item in items:
        identifier = str(item.get("id", ""))
        if identifier not in entrypoints:
            errors.append(f"healthcheck is not an entrypoint: {identifier}")
        if item.get("disposition") not in DISPOSITIONS:
            errors.append(f"invalid healthcheck disposition: {identifier}")
        if not item.get("owner") or not item.get("source") or not item.get("tests"):
            errors.append(f"incomplete healthcheck evidence: {identifier}")
        expected_sources = item.get("configured_sources")
        if not isinstance(expected_sources, Mapping):
            errors.append(f"missing healthcheck source mapping: {identifier}")
            continue
        if set(expected_sources) != set(discovered_configured):
            errors.append(f"healthcheck source set mismatch: {identifier}")
        for source, expected_command in expected_sources.items():
            actual_command = discovered_configured.get(source)
            if actual_command != expected_command:
                errors.append(
                    f"healthcheck command mismatch: {source}: "
                    f"discovered={actual_command!r}, expected={expected_command!r}"
                )
    return errors


def validate_env_inventory(
    discovered: Sequence[str],
    items: Sequence[Mapping[str, Any]],
    policies: Mapping[str, Mapping[str, Any]],
) -> list[str]:
    errors: list[str] = []
    keys = [str(item.get("key", "")) for item in items]
    duplicate_keys = _duplicates(keys)
    if duplicate_keys:
        errors.append(f"duplicate env keys: {sorted(duplicate_keys)}")
    if list(discovered) != keys:
        errors.append(f"env key set/order mismatch: discovered={list(discovered)}, expected={keys}")
    for item in items:
        key = str(item.get("key", ""))
        disposition = item.get("disposition")
        policy_name = item.get("policy")
        policy = policies.get(policy_name) if isinstance(policy_name, str) else None
        if disposition not in DISPOSITIONS:
            errors.append(f"missing/invalid env disposition: {key}")
        if policy is None:
            errors.append(f"missing env policy: {key} -> {policy_name}")
            continue
        if policy.get("disposition") != disposition:
            errors.append(f"env policy disposition mismatch: {key}")
        if not policy.get("owner") or not policy.get("tests"):
            errors.append(f"incomplete env policy: {policy_name}")
    return errors
