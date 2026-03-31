---
name: python
description: Use when writing or reviewing Python code, data scripts, or FastAPI/Django services. Covers type hints, async patterns, modern Python 3.10+ features, and common traps.
license: AGPL-3.0
category: language
icon: 🐍
builtin: true
---

## Procedure

1. **Type hints everywhere**: All function signatures, return types, class attributes. Use `X | Y` syntax (3.10+), not `Union[X, Y]`. Run `mypy --strict`.
2. **Data modeling**: Use `dataclasses` or Pydantic for structured data. Never pass raw dicts as domain objects across function boundaries.
3. **Async**: Use `asyncio` with `async/await`. Use `httpx` (not `requests`) for async HTTP. Never call blocking I/O inside an async function without `asyncio.to_thread()`.
4. **Dependencies**: `pyproject.toml` with pinned versions. Always use virtual environments. Prefer `uv` or `pip-compile` for reproducible installs.
5. **Testing**: pytest with fixtures. Use `@pytest.mark.parametrize` for variations. Mock at boundaries (I/O, network), not internal logic.
6. **Style**: Ruff for linting + formatting (replaces Black, isort, flake8). Google-style docstrings.

## Gotchas

- Mutable default arguments (`def f(items=[])`): the list is shared across calls. Use `None` + create inside.
- `asyncio.run()` cannot be called from inside an already-running event loop — use `await` directly or `asyncio.create_task()`.
- `except Exception` catches `KeyboardInterrupt` in Python < 3.11. Use `except Exception` only, never bare `except:`.
- f-strings in log calls (`log.info(f"x={x}")`) always evaluate — use `log.info("x=%s", x)` for lazy evaluation.

## Validation

Run `ruff check . && mypy --strict . && pytest` before considering work done.

`✓ def fetch_user(user_id: int) -> User | None:`
`✗ def fetch_user(user_id):  # no type hints`
