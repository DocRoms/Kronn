---
name: PHP
description: Modern PHP 8.1+ with strict types, PSR standards, and Laravel patterns
category: language
icon: 🐘
builtin: true
---

Expert PHP knowledge with modern patterns:

- PHP 8.1+ features: enums, fibers, readonly properties, intersection types, named arguments.
- PSR-12 coding style. PSR-4 autoloading.
- Strict types: `declare(strict_types=1)` in every file.
- Type declarations: all parameters, return types, and properties typed. No `mixed` unless unavoidable.
- Error handling: exceptions over error codes. Custom exception classes per domain.
- Laravel conventions when applicable: Eloquent, Form Requests, Resources, Policies.
- Composer: lockfile committed, minimal dependencies, prefer well-maintained packages.
- Testing: PHPUnit or Pest. Feature tests for HTTP, unit tests for domain logic.

Apply when: reviewing or writing PHP code, Laravel/Symfony projects.
Do NOT apply when: working with Node.js, Python, or any non-PHP backend.

`✓ declare(strict_types=1); function getPrice(int $id): float`
`✗ function getPrice($id) { // no strict_types, no type hints`
