# Builds a synthetic PHP/Docker project for the presentation screenshots (scripts/e2e/
# presentation.sh). Runs inside the SFTP test container as the test user:
#   docker exec -i -u gem gem-test-servers-sftp-1 sh -s <dir> <now-epoch> < presentation-seed.sh
# Every name, address and secret in it is made up.
set -eu
ROOT="$1"
NOW="$2"
rm -rf "$ROOT"
mkdir -p "$ROOT" && cd "$ROOT"
mkdir -p app/Http/Controllers app/Http/Middleware app/Models app/Services bootstrap/cache \
  config database/migrations database/seeders public resources/views/invoices routes \
  storage/logs storage/framework/cache tests/Feature tests/Unit vendor/composer

# at <minutes ago> <path…>: sets the modification time.
at() {
  ago="$1"; shift
  touch -d "@$((NOW - ago * 60))" "$@"
}

cat >app/Http/Controllers/InvoiceController.php <<'PHP'
<?php

declare(strict_types=1);

namespace App\Http\Controllers;

use App\Http\Requests\StoreInvoiceRequest;
use App\Models\Invoice;
use App\Services\InvoiceMailer;
use Illuminate\Http\JsonResponse;
use Illuminate\Support\Facades\Log;

/**
 * Invoices of the current account: list, create and send them.
 */
final class InvoiceController extends Controller
{
    private const PER_PAGE = 25;

    public function __construct(private readonly InvoiceMailer $mailer)
    {
    }

    public function index(): JsonResponse
    {
        $invoices = Invoice::query()
            ->where('account_id', auth()->user()->account_id)
            ->latest('issued_at')
            ->paginate(self::PER_PAGE);

        return response()->json($invoices);
    }

    public function store(StoreInvoiceRequest $request): JsonResponse
    {
        $invoice = Invoice::create([
            ...$request->validated(),
            'number' => sprintf('INV-%s-%04d', date('Y'), Invoice::nextSequence()),
            'status' => 'draft',
        ]);

        Log::info('Invoice created', ['id' => $invoice->id, 'total' => $invoice->total]);

        return response()->json($invoice, 201);
    }

    public function send(Invoice $invoice): JsonResponse
    {
        // Only drafts can be sent; a sent invoice is immutable.
        if ($invoice->status !== 'draft') {
            return response()->json(['error' => 'Invoice already sent'], 409);
        }

        $this->mailer->send($invoice);
        $invoice->update(['status' => 'sent', 'sent_at' => now()]);

        return response()->json(['status' => $invoice->status]);
    }
}
PHP

cat >app/Http/Controllers/Controller.php <<'PHP'
<?php

namespace App\Http\Controllers;

abstract class Controller
{
}
PHP

cat >app/Http/Controllers/CustomerController.php <<'PHP'
<?php

declare(strict_types=1);

namespace App\Http\Controllers;

use App\Models\Customer;
use Illuminate\Http\JsonResponse;

final class CustomerController extends Controller
{
    public function show(Customer $customer): JsonResponse
    {
        return response()->json($customer->load('invoices'));
    }
}
PHP

cat >app/Http/Middleware/EnsureAccountIsActive.php <<'PHP'
<?php

namespace App\Http\Middleware;

use Closure;
use Illuminate\Http\Request;

final class EnsureAccountIsActive
{
    public function handle(Request $request, Closure $next): mixed
    {
        abort_unless($request->user()?->account?->active, 403, 'Account suspended');

        return $next($request);
    }
}
PHP

cat >app/Models/Invoice.php <<'PHP'
<?php

namespace App\Models;

use Illuminate\Database\Eloquent\Model;

final class Invoice extends Model
{
    protected $fillable = ['account_id', 'customer_id', 'number', 'status', 'total', 'issued_at'];

    protected $casts = ['issued_at' => 'date', 'sent_at' => 'datetime', 'total' => 'decimal:2'];

    public static function nextSequence(): int
    {
        return (int) static::query()->whereYear('issued_at', date('Y'))->count() + 1;
    }
}
PHP

cat >app/Models/Customer.php <<'PHP'
<?php

namespace App\Models;

use Illuminate\Database\Eloquent\Model;
use Illuminate\Database\Eloquent\Relations\HasMany;

final class Customer extends Model
{
    public function invoices(): HasMany
    {
        return $this->hasMany(Invoice::class);
    }
}
PHP

cat >app/Services/InvoiceMailer.php <<'PHP'
<?php

namespace App\Services;

use App\Models\Invoice;
use Illuminate\Support\Facades\Mail;

final class InvoiceMailer
{
    public function send(Invoice $invoice): void
    {
        Mail::to($invoice->customer->email)->queue(new \App\Mail\InvoiceIssued($invoice));
    }
}
PHP

cat >bootstrap/app.php <<'PHP'
<?php

use Illuminate\Foundation\Application;

return Application::configure(basePath: dirname(__DIR__))
    ->withRouting(api: __DIR__ . '/../routes/api.php', web: __DIR__ . '/../routes/web.php')
    ->create();
PHP
: >bootstrap/cache/.gitignore

cat >config/app.php <<'PHP'
<?php

return [
    'name' => env('APP_NAME', 'Acme API'),
    'env' => env('APP_ENV', 'production'),
    'debug' => (bool) env('APP_DEBUG', false),
    'url' => env('APP_URL', 'https://api.acme.example'),
    'timezone' => 'UTC',
];
PHP
cat >config/database.php <<'PHP'
<?php

return [
    'default' => env('DB_CONNECTION', 'pgsql'),
    'connections' => [
        'pgsql' => [
            'host' => env('DB_HOST', 'db'),
            'port' => env('DB_PORT', '5432'),
            'database' => env('DB_DATABASE', 'acme'),
        ],
    ],
];
PHP
cat >config/mail.php <<'PHP'
<?php

return ['default' => env('MAIL_MAILER', 'smtp'), 'from' => ['address' => 'billing@acme.example']];
PHP

cat >database/migrations/2026_09_14_101500_create_invoices_table.php <<'PHP'
<?php

use Illuminate\Database\Migrations\Migration;
use Illuminate\Database\Schema\Blueprint;
use Illuminate\Support\Facades\Schema;

return new class extends Migration
{
    public function up(): void
    {
        Schema::create('invoices', function (Blueprint $table) {
            $table->id();
            $table->foreignId('customer_id')->constrained();
            $table->string('number')->unique();
            $table->string('status')->default('draft');
            $table->decimal('total', 10, 2);
            $table->date('issued_at');
            $table->timestamps();
        });
    }
};
PHP
cat >database/seeders/DatabaseSeeder.php <<'PHP'
<?php

namespace Database\Seeders;

use Illuminate\Database\Seeder;

final class DatabaseSeeder extends Seeder
{
    public function run(): void
    {
        $this->call([CustomerSeeder::class, InvoiceSeeder::class]);
    }
}
PHP

cat >public/index.php <<'PHP'
<?php

define('LARAVEL_START', microtime(true));

require __DIR__ . '/../vendor/autoload.php';

(require_once __DIR__ . '/../bootstrap/app.php')->handleRequest(Illuminate\Http\Request::capture());
PHP
printf 'User-agent: *\nDisallow: /\n' >public/robots.txt
cat >resources/views/invoices/show.blade.php <<'HTML'
<h1>Invoice {{ $invoice->number }}</h1>
<p>Total: {{ number_format($invoice->total, 2) }} EUR</p>
HTML

cat >routes/api.php <<'PHP'
<?php

use App\Http\Controllers\CustomerController;
use App\Http\Controllers\InvoiceController;
use Illuminate\Support\Facades\Route;

Route::middleware(['auth:sanctum', 'account.active'])->group(function () {
    Route::get('/invoices', [InvoiceController::class, 'index']);
    Route::post('/invoices', [InvoiceController::class, 'store']);
    Route::post('/invoices/{invoice}/send', [InvoiceController::class, 'send']);
    Route::get('/customers/{customer}', [CustomerController::class, 'show']);
});
PHP
cat >routes/web.php <<'PHP'
<?php

use Illuminate\Support\Facades\Route;

Route::get('/', fn () => response()->json(['service' => 'acme-api', 'status' => 'ok']));
PHP

# A believable application log (the "Tail app log" snippet shows its last 40 lines).
i=0
: >storage/logs/app.log
while [ "$i" -lt 160 ]; do
  t=$((NOW - (160 - i) * 37))
  stamp="$(date -u -d "@$t" '+%Y-%m-%d %H:%M:%S')"
  case $((i % 9)) in
    0) line="local.INFO: Invoice created {\"id\":$((4100 + i)),\"total\":\"$((i * 13 % 900 + 49)).00\"}" ;;
    1) line="local.INFO: GET /api/invoices 200 $((i % 40 + 12))ms" ;;
    2) line="local.INFO: Mail queued {\"to\":\"billing-$((i % 7))@customer.example\",\"invoice\":$((4099 + i))}" ;;
    3) line="local.DEBUG: Cache hit invoices.page.$((i % 5 + 1))" ;;
    4) line="local.INFO: POST /api/invoices/$((4090 + i))/send 200 $((i % 60 + 30))ms" ;;
    5) line="local.WARNING: Slow query (412ms) select * from \"invoices\" where \"account_id\" = ?" ;;
    6) line="local.INFO: GET /api/customers/$((i % 30 + 1)) 200 $((i % 20 + 8))ms" ;;
    7) line="local.INFO: Queue worker processed App\\Jobs\\SendInvoice in $((i % 300 + 90))ms" ;;
    *) line="local.INFO: Health check ok {\"db\":\"up\",\"queue\":\"up\"}" ;;
  esac
  printf '[%s] %s\n' "$stamp" "$line" >>storage/logs/app.log
  i=$((i + 1))
done
: >storage/framework/cache/.gitignore

cat >tests/Feature/InvoiceTest.php <<'PHP'
<?php

namespace Tests\Feature;

use App\Models\Invoice;
use Tests\TestCase;

final class InvoiceTest extends TestCase
{
    public function test_a_sent_invoice_cannot_be_sent_again(): void
    {
        $invoice = Invoice::factory()->create(['status' => 'sent']);

        $this->postJson("/api/invoices/{$invoice->id}/send")->assertStatus(409);
    }
}
PHP
cat >tests/Unit/InvoiceNumberTest.php <<'PHP'
<?php

namespace Tests\Unit;

use PHPUnit\Framework\TestCase;

final class InvoiceNumberTest extends TestCase
{
    public function test_numbers_are_padded(): void
    {
        $this->assertSame('INV-2026-0042', sprintf('INV-%s-%04d', '2026', 42));
    }
}
PHP
cat >vendor/autoload.php <<'PHP'
<?php

require_once __DIR__ . '/composer/autoload_real.php';

return ComposerAutoloaderInit::getLoader();
PHP
printf '<?php\n\nclass ComposerAutoloaderInit\n{\n}\n' >vendor/composer/autoload_real.php

# ---- the project root ----
cat >.env <<'ENV'
# Synthetic environment for the Gem screenshots. Not a real secret.
APP_NAME="Acme API"
APP_ENV=staging
APP_DEBUG=false
APP_URL=https://api.acme.example
DB_CONNECTION=pgsql
DB_HOST=db
DB_PORT=5432
DB_DATABASE=acme
DB_USERNAME=acme
DB_PASSWORD="not-a-real-password"
MAIL_MAILER=smtp
QUEUE_CONNECTION=redis
ENV
sed 's/="not-a-real-password"/=/; s/^APP_ENV=staging/APP_ENV=local/' .env >.env.example
printf '/vendor/\n/node_modules/\n.env\n/storage/*.key\n/public/hot\n.phpunit.result.cache\n' >.gitignore
cat >artisan <<'PHP'
#!/usr/bin/env php
<?php

define('LARAVEL_START', microtime(true));

require __DIR__ . '/vendor/autoload.php';

$status = (require_once __DIR__ . '/bootstrap/app.php')->handleCommand(new Symfony\Component\Console\Input\ArgvInput());

exit($status);
PHP
chmod 755 artisan
cat >composer.json <<'JSON'
{
    "name": "acme/acme-api",
    "description": "Invoicing API (synthetic project for the Gem screenshots)",
    "type": "project",
    "license": "proprietary",
    "require": {
        "php": "^8.3",
        "laravel/framework": "^12.0",
        "laravel/sanctum": "^4.0",
        "predis/predis": "^2.3"
    },
    "require-dev": {
        "phpunit/phpunit": "^11.5",
        "laravel/pint": "^1.18"
    },
    "autoload": {
        "psr-4": {
            "App\\": "app/",
            "Database\\Seeders\\": "database/seeders/"
        }
    },
    "scripts": {
        "test": "phpunit"
    },
    "config": {
        "sort-packages": true
    }
}
JSON
# A lock file of a realistic size.
{
  printf '{\n    "_readme": ["This file locks the dependencies of your project to a known state"],\n    "content-hash": "4f1c2a9e7b3d5f60a8c1e2d3b4a59687",\n    "packages": [\n'
  n=0
  for pkg in brick/math carbonphp/carbon-doctrine-types dflydev/dot-access-data doctrine/inflector \
    doctrine/lexer dragonmantank/cron-expression egulias/email-validator fruitcake/php-cors \
    graham-campbell/result-type guzzlehttp/guzzle guzzlehttp/promises guzzlehttp/psr7 \
    laravel/framework laravel/prompts laravel/sanctum laravel/serializable-closure league/commonmark \
    league/flysystem league/mime-type-detection monolog/monolog nesbot/carbon nette/schema \
    nette/utils nunomaduro/termwind phpoption/phpoption predis/predis psr/clock psr/container \
    psr/event-dispatcher psr/http-client psr/http-factory psr/http-message psr/log psr/simple-cache \
    ramsey/collection ramsey/uuid symfony/console symfony/css-selector symfony/error-handler \
    symfony/event-dispatcher symfony/finder symfony/http-foundation symfony/http-kernel \
    symfony/mailer symfony/mime symfony/process symfony/routing symfony/string symfony/translation \
    symfony/uid symfony/var-dumper vlucas/phpdotenv voku/portable-ascii; do
    [ "$n" -gt 0 ] && printf ',\n'
    n=$((n + 1))
    printf '        {\n            "name": "%s",\n            "version": "v%d.%d.%d",\n' "$pkg" $((n % 7 + 1)) $((n * 3 % 10)) $((n * 7 % 13))
    printf '            "source": {"type": "git", "url": "https://git.example/%s.git", "reference": "%08x"},\n' "$pkg" $((n * 2654435761 % 4294967296))
    printf '            "require": {"php": ">=8.2"},\n            "type": "library",\n            "license": ["MIT"]\n        }'
  done
  printf '\n    ],\n    "packages-dev": [],\n    "minimum-stability": "stable",\n    "platform": {"php": "^8.3"}\n}\n'
} >composer.lock
cat >docker-compose.yml <<'YAML'
services:
  app:
    build: .
    image: acme/api:latest
    ports:
      - "127.0.0.1:8080:8080"
    environment:
      APP_ENV: staging
      DB_HOST: db
      REDIS_HOST: redis
    volumes:
      - ./storage:/var/www/html/storage
    depends_on:
      db:
        condition: service_healthy
      redis:
        condition: service_started
  worker:
    image: acme/api:latest
    command: php artisan queue:work --tries=3
    depends_on: [app]
  db:
    image: postgres:17-alpine
    environment:
      POSTGRES_DB: acme
      POSTGRES_USER: acme
      POSTGRES_PASSWORD: acme
    volumes:
      - pgdata:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD", "pg_isready", "-U", "acme"]
      interval: 5s
  redis:
    image: redis:7-alpine
volumes:
  pgdata: {}
YAML
cat >Dockerfile <<'DOCKER'
# syntax=docker/dockerfile:1
FROM composer:2 AS vendor
WORKDIR /app
COPY composer.json composer.lock ./
RUN composer install --no-dev --no-scripts --prefer-dist

FROM php:8.3-fpm-alpine
RUN apk add --no-cache postgresql-dev && docker-php-ext-install pdo_pgsql opcache
WORKDIR /var/www/html
COPY --from=vendor /app/vendor ./vendor
COPY . .
RUN chown -R www-data:www-data storage bootstrap/cache
USER www-data
EXPOSE 8080
CMD ["php", "artisan", "serve", "--host=0.0.0.0", "--port=8080"]
DOCKER
cat >Makefile <<'MAKE'
.PHONY: up down test migrate logs

up:
	docker compose up -d --build

down:
	docker compose down

test:
	docker compose exec app php artisan test

migrate:
	docker compose exec app php artisan migrate --force

logs:
	tail -f storage/logs/app.log
MAKE
cat >phpunit.xml <<'XML'
<?xml version="1.0" encoding="UTF-8"?>
<phpunit bootstrap="vendor/autoload.php" colors="true">
    <testsuites>
        <testsuite name="Unit">
            <directory>tests/Unit</directory>
        </testsuite>
        <testsuite name="Feature">
            <directory>tests/Feature</directory>
        </testsuite>
    </testsuites>
    <php>
        <env name="APP_ENV" value="testing"/>
        <env name="DB_CONNECTION" value="sqlite"/>
        <env name="DB_DATABASE" value=":memory:"/>
    </php>
</phpunit>
XML
cat >README.md <<'MD'
# Acme API

Invoicing API for the Acme customer portal (a synthetic project).

## Run locally

```sh
make up
make migrate
make test
```

| Service  | Port | Purpose          |
| -------- | ---- | ---------------- |
| `app`    | 8080 | HTTP API         |
| `worker` | —    | Queued mail jobs |
| `db`     | 5432 | PostgreSQL       |
MD

# Varied, believable modification times (minutes ago).
find . -exec touch -d "@$((NOW - 40 * 24 * 60))" {} +
at 9    app app/Http app/Http/Controllers app/Http/Controllers/InvoiceController.php
at 2900 app/Http/Controllers/CustomerController.php app/Models app/Models/Invoice.php
at 26   storage storage/logs
at 3    storage/logs/app.log
at 4330 bootstrap config database database/migrations
at 95   routes routes/api.php
at 14500 public resources tests vendor
at 52   .env
at 21000 .env.example .gitignore artisan phpunit.xml
at 7200 composer.json composer.lock
at 140  docker-compose.yml
at 11600 Dockerfile Makefile
at 1500 README.md
