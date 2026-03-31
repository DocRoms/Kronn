---
name: php
description: Use when writing or reviewing PHP code or Laravel/Symfony projects. Covers strict types, PHP 8.1+ features, PSR standards, and common PHP traps.
license: AGPL-3.0
category: language
icon: 🐘
builtin: true
---

## Procedure

1. **Strict types**: `declare(strict_types=1);` at the top of every file. Type all parameters, return types, and properties. Avoid `mixed` unless truly needed.
2. **PHP 8.1+**: Use enums for fixed value sets, readonly properties for DTOs, named arguments for clarity, intersection types for precise contracts.
3. **Laravel**: Use Form Requests for validation (not inline `$request->validate()`). Use Resources for API responses. Use Policies for authorization.
4. **Error handling**: Throw domain-specific exceptions. Never catch `\Exception` broadly — catch the specific type and let unexpected errors bubble.
5. **Dependencies**: Commit `composer.lock`. Minimal dependencies. Prefer well-maintained packages with active security support.
6. **Testing**: PHPUnit or Pest. Feature tests for HTTP endpoints, unit tests for domain logic. Use database transactions for test isolation.

## Gotchas

- Loose comparison (`==`) has surprising coercions: `"0" == false` is `true`, `"" == 0` is `true`. Always use `===`.
- `array_merge()` in a loop is O(n^2) — collect arrays and merge once, or use spread: `array_merge(...$arrays)`.
- Laravel `firstOrCreate()` is not atomic without a unique index — you'll get duplicates under concurrency.
- `nullable` vs `optional` in Laravel validation: `nullable` allows null values, `sometimes` skips validation if the field is absent. They are not interchangeable.

## Validation

Run `php artisan test` or `./vendor/bin/phpunit` and `phpstan analyse` before considering work done.

`✓ declare(strict_types=1); function getPrice(int $id): float`
`✗ function getPrice($id) { // no strict_types, no type hints`
