/** Contents of the demo remote tree served by the dev mock (synthetic data only). */
export const SAMPLE_FILES: Record<string, string> = {
  '/var/www/api/index.php': `<?php

declare(strict_types=1);

/**
 * Front controller for the Acme API.
 *
 * Every request that does not match a static file is rewritten here by the
 * web server (see public/.htaccess and the nginx site config).
 */

use App\\Http\\Kernel;
use App\\Support\\ErrorHandler;
use Symfony\\Component\\HttpFoundation\\Request;

define('APP_START', microtime(true));
define('BASE_PATH', __DIR__);

// Composer autoloader; fail loudly if dependencies were never installed.
$autoload = BASE_PATH . '/vendor/autoload.php';
if (!is_file($autoload)) {
    http_response_code(500);
    header('Content-Type: text/plain; charset=utf-8');
    echo "Dependencies missing. Run \`composer install --no-dev\` first.\\n";
    exit(1);
}
require $autoload;

// Maintenance mode: a file dropped by deploy.sh while migrations run.
$maintenance = BASE_PATH . '/storage/framework/down';
if (file_exists($maintenance)) {
    $payload = json_decode((string) file_get_contents($maintenance), true) ?? [];
    $retry = (int) ($payload['retry'] ?? 60);

    http_response_code(503);
    header('Retry-After: ' . $retry);
    header('Content-Type: application/json');
    echo json_encode(['message' => 'Service temporarily unavailable', 'retry' => $retry]);
    exit;
}

$app = require_once BASE_PATH . '/bootstrap/app.php';

ErrorHandler::register(
    debug: filter_var($_ENV['APP_DEBUG'] ?? false, FILTER_VALIDATE_BOOLEAN),
    logPath: BASE_PATH . '/storage/logs/laravel.log',
);

/** @var Kernel $kernel */
$kernel = $app->make(Kernel::class);

$request = Request::createFromGlobals();
$response = $kernel->handle($request);

$elapsed = round((microtime(true) - APP_START) * 1000, 2);
$response->headers->set('X-Response-Time', "{$elapsed}ms");
$response->headers->set('X-Request-Id', $request->headers->get('X-Request-Id', bin2hex(random_bytes(8))));

$response->send();

$kernel->terminate($request, $response);
`,
  '/var/www/api/app/Http/Controllers/UserController.php': `<?php

namespace App\\Http\\Controllers;

use App\\Http\\Requests\\StoreUserRequest;
use App\\Http\\Resources\\UserResource;
use App\\Models\\User;
use Illuminate\\Http\\JsonResponse;
use Illuminate\\Http\\Request;
use Illuminate\\Http\\Resources\\Json\\AnonymousResourceCollection;
use Illuminate\\Support\\Facades\\Cache;
use Illuminate\\Support\\Facades\\Hash;
use Illuminate\\Support\\Facades\\Log;

final class UserController extends Controller
{
    private const PER_PAGE = 25;
    private const CACHE_TTL = 300; // seconds

    public function __construct()
    {
        $this->middleware('auth:sanctum');
        $this->middleware('throttle:60,1')->only(['store', 'update']);
    }

    /**
     * List users, optionally filtered by role and a free-text search.
     */
    public function index(Request $request): AnonymousResourceCollection
    {
        $users = User::query()
            ->when($request->string('role')->isNotEmpty(), fn ($q) => $q->where('role', $request->input('role')))
            ->when($request->filled('q'), function ($q) use ($request) {
                $term = '%' . str_replace(['%', '_'], ['\\%', '\\_'], $request->input('q')) . '%';
                $q->where(fn ($w) => $w->where('name', 'like', $term)->orWhere('email', 'like', $term));
            })
            ->orderByDesc('created_at')
            ->paginate(min((int) $request->input('per_page', self::PER_PAGE), 100));

        return UserResource::collection($users);
    }

    public function show(int $id): UserResource
    {
        $user = Cache::remember("users:{$id}", self::CACHE_TTL, fn () => User::with('teams')->findOrFail($id));

        return new UserResource($user);
    }

    public function store(StoreUserRequest $request): JsonResponse
    {
        $data = $request->validated();
        $data['password'] = Hash::make($data['password']);

        $user = User::create($data);
        Log::info('User created', ['id' => $user->id, 'by' => $request->user()?->id]);

        return (new UserResource($user))
            ->response()
            ->setStatusCode(201)
            ->header('Location', route('users.show', $user));
    }

    public function update(Request $request, User $user): UserResource
    {
        $this->authorize('update', $user);

        $validated = $request->validate([
            'name'  => ['sometimes', 'string', 'max:120'],
            'email' => ['sometimes', 'email', "unique:users,email,{$user->id}"],
            'role'  => ['sometimes', 'in:admin,editor,viewer'],
        ]);

        $user->fill($validated)->save();
        Cache::forget("users:{$user->id}");

        return new UserResource($user->refresh());
    }

    public function destroy(User $user): JsonResponse
    {
        $this->authorize('delete', $user);

        if ($user->isLastAdmin()) {
            return response()->json(['message' => 'Cannot delete the last administrator.'], 409);
        }

        $user->delete();
        Cache::forget("users:{$user->id}");

        return response()->json(null, 204);
    }
}
`,
  '/var/www/api/app/Models/User.php': `<?php

namespace App\\Models;

use Illuminate\\Database\\Eloquent\\Casts\\Attribute;
use Illuminate\\Database\\Eloquent\\Factories\\HasFactory;
use Illuminate\\Database\\Eloquent\\Relations\\BelongsToMany;
use Illuminate\\Database\\Eloquent\\Relations\\HasMany;
use Illuminate\\Database\\Eloquent\\SoftDeletes;
use Illuminate\\Foundation\\Auth\\User as Authenticatable;
use Illuminate\\Notifications\\Notifiable;
use Laravel\\Sanctum\\HasApiTokens;

/**
 * @property int $id
 * @property string $name
 * @property string $email
 * @property string $role
 * @property \\Carbon\\Carbon|null $email_verified_at
 */
class User extends Authenticatable
{
    use HasApiTokens, HasFactory, Notifiable, SoftDeletes;

    public const ROLE_ADMIN = 'admin';
    public const ROLE_EDITOR = 'editor';
    public const ROLE_VIEWER = 'viewer';

    protected $fillable = ['name', 'email', 'password', 'role', 'timezone'];

    protected $hidden = ['password', 'remember_token'];

    protected $attributes = [
        'role' => self::ROLE_VIEWER,
        'timezone' => 'UTC',
    ];

    protected function casts(): array
    {
        return [
            'email_verified_at' => 'datetime',
            'last_login_at' => 'datetime',
            'password' => 'hashed',
            'settings' => 'array',
        ];
    }

    public function teams(): BelongsToMany
    {
        return $this->belongsToMany(Team::class)->withPivot('role')->withTimestamps();
    }

    public function apiKeys(): HasMany
    {
        return $this->hasMany(ApiKey::class);
    }

    protected function initials(): Attribute
    {
        return Attribute::get(function (): string {
            $parts = preg_split('/\\s+/', trim($this->name)) ?: [];
            return strtoupper(implode('', array_map(fn ($p) => $p[0] ?? '', array_slice($parts, 0, 2))));
        });
    }

    public function isAdmin(): bool
    {
        return $this->role === self::ROLE_ADMIN;
    }

    public function isLastAdmin(): bool
    {
        return $this->isAdmin() && static::where('role', self::ROLE_ADMIN)->count() === 1;
    }

    public function scopeActive($query)
    {
        return $query->whereNotNull('email_verified_at')->where('last_login_at', '>=', now()->subDays(90));
    }
}
`,
  '/var/www/api/config/app.php': `<?php

use Illuminate\\Support\\Facades\\Facade;

return [

    /*
    |--------------------------------------------------------------------------
    | Application Name & Environment
    |--------------------------------------------------------------------------
    */

    'name' => env('APP_NAME', 'Acme API'),

    'env' => env('APP_ENV', 'production'),

    'debug' => (bool) env('APP_DEBUG', false),

    'url' => env('APP_URL', 'https://api.example.com'),

    'asset_url' => env('ASSET_URL'),

    /*
    |--------------------------------------------------------------------------
    | Localization
    |--------------------------------------------------------------------------
    */

    'timezone' => 'UTC',

    'locale' => 'en',

    'fallback_locale' => 'en',

    'supported_locales' => ['en', 'fr', 'pt', 'de'],

    /*
    |--------------------------------------------------------------------------
    | Encryption Key
    |--------------------------------------------------------------------------
    | Generated by \`php artisan key:generate\`. Never commit a real key.
    */

    'key' => env('APP_KEY', 'base64:Zm9vYmFyYmF6cXV4Y2hhbmdlbWVjaGFuZ2VtZQ=='),

    'cipher' => 'AES-256-CBC',

    'previous_keys' => array_filter(explode(',', (string) env('APP_PREVIOUS_KEYS', ''))),

    /*
    |--------------------------------------------------------------------------
    | Limits
    |--------------------------------------------------------------------------
    */

    'limits' => [
        'upload_max_mb' => (int) env('UPLOAD_MAX_MB', 64),
        'export_rows' => 50_000,
        'api_rate_per_minute' => 120,
        'session_lifetime' => 60 * 24, // minutes
    ],

    'maintenance' => [
        'driver' => 'file',
        'store' => 'redis',
    ],

    'providers' => [
        App\\Providers\\AppServiceProvider::class,
        App\\Providers\\AuthServiceProvider::class,
        App\\Providers\\EventServiceProvider::class,
        App\\Providers\\RouteServiceProvider::class,
    ],

    'aliases' => Facade::defaultAliases()->merge([
        'Feature' => App\\Support\\Facades\\Feature::class,
    ])->toArray(),

];
`,
  '/var/www/api/config.yaml': `# Acme API runtime configuration
# Values here are defaults; anything in .env takes precedence.

server:
  port: 8080
  host: 0.0.0.0
  read_timeout: 15s
  write_timeout: 30s
  idle_timeout: 120s
  max_body_size: 64MB
  trusted_proxies:
    - 10.0.0.0/8
    - 203.0.113.10
  tls:
    enabled: false
    cert_file: /etc/ssl/certs/api.example.com.pem
    key_file: /etc/ssl/private/api.example.com.key

database:
  driver: mysql
  host: 10.0.0.21
  port: 3306
  name: acme
  user: acme_app
  password: change-me
  pool:
    max_open: 25
    max_idle: 5
    max_lifetime: 30m
  replicas:
    - host: 10.0.0.22
      weight: 2
    - host: 10.0.0.23
      weight: 1

cache:
  driver: redis
  url: redis://10.0.0.31:6379/0
  prefix: "acme:"
  default_ttl: 300
  compress: true

queue:
  connection: redis
  workers: 4
  retry_after: 90
  queues: [default, mail, exports]

logging:
  level: info          # debug | info | warn | error
  format: json
  outputs:
    - type: file
      path: /var/www/api/storage/logs/app.log
      rotate: daily
      keep: 14
    - type: stderr
  redact:
    - password
    - authorization
    - "*.token"

features:
  new_billing: false
  beta_exports: true
  rate_limit_per_minute: 120

mail:
  from: "Acme <no-reply@example.com>"
  smtp:
    host: smtp.example.com
    port: 587
    encryption: tls
`,
  '/var/www/api/config/database.yaml': `# Database connections, read by both the PHP app and the Go sidecar.
default: mysql

connections:
  mysql:
    driver: mysql
    host: \${DB_HOST:-10.0.0.21}
    port: 3306
    database: acme
    username: acme_app
    password: change-me
    charset: utf8mb4
    collation: utf8mb4_unicode_ci
    strict: true
    options:
      connect_timeout: 5
      ssl_mode: PREFERRED

  pgsql:
    driver: pgsql
    host: 10.0.0.24
    port: 5432
    database: acme_analytics
    username: analytics_ro
    password: change-me
    search_path: public
    sslmode: require

  sqlite:
    driver: sqlite
    database: storage/testing.sqlite
    foreign_keys: true

migrations:
  table: schema_migrations
  path: db/migrations
  lock_timeout: 30

redis:
  client: phpredis
  default:
    host: 10.0.0.31
    port: 6379
    database: 0
  cache:
    host: 10.0.0.31
    port: 6379
    database: 1

backups:
  enabled: true
  schedule: "15 3 * * *"
  retention_days: 21
  destination: s3://backups.example.test/acme/
`,
  '/var/www/api/main.go': `// Command sidecar exposes health and metrics endpoints next to the PHP API.
package main

import (
	"cmp"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"net/http"
	"os"
	"os/signal"
	"sync"
	"syscall"
	"time"
)

const defaultAddr = ":8081"

type Status struct {
	Service string            \`json:"service"\`
	Version string            \`json:"version"\`
	Uptime  string            \`json:"uptime"\`
	Checks  map[string]string \`json:"checks"\`
	Healthy bool              \`json:"healthy"\`
}

type Counter struct {
	mu     sync.Mutex
	counts map[string]int64
}

func NewCounter() *Counter {
	return &Counter{counts: make(map[string]int64)}
}

func (c *Counter) Inc(path string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.counts[path]++
}

var version, startedAt = "1.8.3", time.Now()
var errDown = errors.New("dependency unavailable")

func checkTCP(ctx context.Context, addr string) error {
	ctx, cancel := context.WithTimeout(ctx, 2*time.Second)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, "http://"+addr+"/ping", nil)
	if err != nil {
		return err
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return fmt.Errorf("%w: %v", errDown, err)
	}
	resp.Body.Close()
	return nil
}

func healthHandler(w http.ResponseWriter, r *http.Request) {
	st := Status{
		Service: "acme-sidecar",
		Version: version,
		Uptime:  time.Since(startedAt).Round(time.Second).String(),
		Checks:  map[string]string{},
		Healthy: true,
	}
	st.Checks["redis"] = "ok"
	if err := checkTCP(r.Context(), "10.0.0.31:6380"); err != nil {
		st.Checks["redis"], st.Healthy = err.Error(), false
	}
	w.Header().Set("Content-Type", "application/json")
	if !st.Healthy {
		w.WriteHeader(http.StatusServiceUnavailable)
	}
	_ = json.NewEncoder(w).Encode(st)
}

func withCounting(c *Counter, next http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		c.Inc(r.URL.Path)
		next(w, r)
	}
}

func main() {
	addr := cmp.Or(os.Getenv("SIDECAR_ADDR"), defaultAddr)

	counter := NewCounter()
	mux := http.NewServeMux()
	mux.HandleFunc("/healthz", withCounting(counter, healthHandler))
	mux.HandleFunc("/metrics", func(w http.ResponseWriter, _ *http.Request) {
		counter.mu.Lock()
		defer counter.mu.Unlock()
		for path, n := range counter.counts {
			fmt.Fprintf(w, "sidecar_requests_total{path=%q} %d\\n", path, n)
		}
	})

	srv := &http.Server{Addr: addr, Handler: mux, ReadHeaderTimeout: 5 * time.Second}

	go func() {
		log.Printf("sidecar %s listening on %s", version, addr)
		if err := srv.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
			log.Fatalf("listen: %v", err)
		}
	}()

	stop := make(chan os.Signal, 1)
	signal.Notify(stop, syscall.SIGINT, syscall.SIGTERM)
	<-stop

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if err := srv.Shutdown(ctx); err != nil {
		log.Printf("shutdown error: %v", err)
	}
}
`,
  '/var/www/api/go.mod': `module example.com/acme/sidecar

go 1.23

toolchain go1.23.4

require (
	github.com/go-chi/chi/v5 v5.1.0
	github.com/prometheus/client_golang v1.20.5
	github.com/redis/go-redis/v9 v9.7.0
	gopkg.in/yaml.v3 v3.0.1
)

require (
	github.com/beorn7/perks v1.0.1 // indirect
	github.com/cespare/xxhash/v2 v2.3.0 // indirect
	github.com/dgryski/go-rendezvous v0.0.0-20200823014737-9f7001d12a5f // indirect
	github.com/klauspost/compress v1.17.9 // indirect
	github.com/munnerz/goautoneg v0.0.0-20191010083416-a7dc8b61c822 // indirect
	github.com/prometheus/client_model v0.6.1 // indirect
	github.com/prometheus/common v0.55.0 // indirect
	github.com/prometheus/procfs v0.15.1 // indirect
	golang.org/x/sys v0.22.0 // indirect
	google.golang.org/protobuf v1.34.2 // indirect
)

// Pin until the upstream fix for the pool leak is released.
replace github.com/redis/go-redis/v9 => github.com/redis/go-redis/v9 v9.6.1

exclude github.com/klauspost/compress v1.17.8
`,
  '/var/www/api/composer.json': `{
    "name": "acme/api",
    "type": "project",
    "description": "Acme REST API (Laravel)",
    "keywords": ["api", "laravel", "acme"],
    "license": "proprietary",
    "require": {
        "php": "^8.3",
        "guzzlehttp/guzzle": "^7.9",
        "laravel/framework": "^11.30",
        "laravel/sanctum": "^4.0",
        "league/flysystem-aws-s3-v3": "^3.29",
        "monolog/monolog": "^3.7"
    },
    "require-dev": {
        "fakerphp/faker": "^1.23",
        "laravel/pint": "^1.18",
        "mockery/mockery": "^1.6",
        "phpstan/phpstan": "^1.12",
        "phpunit/phpunit": "^11.4"
    },
    "autoload": {
        "psr-4": {
            "App\\\\": "app/",
            "Database\\\\Factories\\\\": "database/factories/",
            "Database\\\\Seeders\\\\": "database/seeders/"
        },
        "files": ["app/helpers.php"]
    },
    "autoload-dev": {
        "psr-4": {
            "Tests\\\\": "tests/"
        }
    },
    "scripts": {
        "post-autoload-dump": [
            "Illuminate\\\\Foundation\\\\ComposerScripts::postAutoloadDump",
            "@php artisan package:discover --ansi"
        ],
        "test": "phpunit --colors=always",
        "analyse": "phpstan analyse --memory-limit=1G",
        "lint": "pint --test"
    },
    "config": {
        "optimize-autoloader": true,
        "preferred-install": "dist",
        "sort-packages": true,
        "platform": {
            "php": "8.3.12"
        },
        "allow-plugins": {
            "php-http/discovery": true
        }
    },
    "minimum-stability": "stable",
    "prefer-stable": true
}
`,
  '/var/www/api/composer.lock': `{
    "_readme": [
        "This file locks the dependencies of your project to a known state",
        "Read more about it at https://getcomposer.org/doc/01-basic-usage.md#installing-dependencies",
        "This file is @generated automatically"
    ],
    "content-hash": "3f9c2a7d41e85b06c1d94e8a7f2b63d0",
    "packages": [
        {
            "name": "guzzlehttp/guzzle",
            "version": "7.9.2",
            "source": {
                "type": "git",
                "url": "https://git.example.test/mirror/guzzle.git",
                "reference": "d281ed313b989f213357e3be1a179f02196ac99b"
            },
            "dist": {
                "type": "zip",
                "url": "https://packages.example.test/dist/guzzlehttp/guzzle/7.9.2.zip",
                "reference": "d281ed313b989f213357e3be1a179f02196ac99b",
                "shasum": ""
            },
            "require": {
                "ext-json": "*",
                "guzzlehttp/promises": "^1.5.3 || ^2.0.3",
                "guzzlehttp/psr7": "^2.7.0",
                "php": "^7.2.5 || ^8.0"
            },
            "type": "library",
            "autoload": {
                "files": ["src/functions_include.php"],
                "psr-4": {
                    "GuzzleHttp\\\\": "src/"
                }
            },
            "license": ["MIT"],
            "description": "Guzzle is a PHP HTTP client library",
            "time": "2024-07-24T11:22:20+00:00"
        },
        {
            "name": "monolog/monolog",
            "version": "3.7.0",
            "source": {
                "type": "git",
                "url": "https://git.example.test/mirror/monolog.git",
                "reference": "f4393b648b78a5408747de94fca38beb5f7e9ef8"
            },
            "require": {
                "php": ">=8.1",
                "psr/log": "^2.0 || ^3.0"
            },
            "provide": {
                "psr/log-implementation": "3.0.0"
            },
            "type": "library",
            "license": ["MIT"],
            "description": "Sends your logs to files, sockets, inboxes, databases and various web services",
            "time": "2024-06-28T09:40:51+00:00"
        },
        {
            "name": "psr/log",
            "version": "3.0.2",
            "source": {
                "type": "git",
                "url": "https://git.example.test/mirror/psr-log.git",
                "reference": "f16e1d5863e37f8d8c2a01719f5b34baa2b714d3"
            },
            "require": {
                "php": ">=8.0.0"
            },
            "type": "library",
            "license": ["MIT"],
            "description": "Common interface for logging libraries",
            "time": "2024-09-11T13:17:53+00:00"
        }
    ],
    "packages-dev": [],
    "aliases": [],
    "minimum-stability": "stable",
    "stability-flags": {},
    "prefer-stable": true,
    "prefer-lowest": false,
    "platform": {
        "php": "^8.3"
    },
    "platform-dev": {},
    "plugin-api-version": "2.6.0"
}
`,
  '/var/www/api/package.json': `{
  "name": "@acme/api-frontend",
  "version": "1.8.3",
  "private": true,
  "type": "module",
  "description": "Admin dashboard and asset pipeline for the Acme API",
  "engines": {
    "node": ">=20.11"
  },
  "scripts": {
    "dev": "vite --host 0.0.0.0 --port 5173",
    "build": "tsc --noEmit && vite build",
    "preview": "vite preview",
    "test": "vitest run",
    "test:watch": "vitest",
    "lint": "eslint . --max-warnings=0",
    "format": "prettier --write \\"src/**/*.{ts,tsx,css}\\"",
    "serve:api": "tsx src/server.ts"
  },
  "dependencies": {
    "react": "^18.3.1",
    "react-dom": "^18.3.1",
    "zod": "^3.23.8"
  },
  "devDependencies": {
    "@types/node": "^22.7.5",
    "@types/react": "^18.3.11",
    "@types/react-dom": "^18.3.1",
    "@vitejs/plugin-react": "^4.3.2",
    "eslint": "^9.12.0",
    "prettier": "^3.3.3",
    "sass": "^1.79.4",
    "tsx": "^4.19.1",
    "typescript": "^5.6.3",
    "vite": "^5.4.8",
    "vitest": "^2.1.2"
  },
  "browserslist": [
    "> 0.5%",
    "last 2 versions",
    "not dead"
  ]
}
`,
  '/var/www/api/tsconfig.json': `{
  "$schema": "https://json.schemastore.org/tsconfig",
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "lib": ["ES2022", "DOM", "DOM.Iterable"],
    "jsx": "react-jsx",
    "strict": true,
    "noUncheckedIndexedAccess": true,
    "noImplicitOverride": true,
    "noFallthroughCasesInSwitch": true,
    "exactOptionalPropertyTypes": false,
    "esModuleInterop": true,
    "resolveJsonModule": true,
    "isolatedModules": true,
    "skipLibCheck": true,
    "sourceMap": true,
    "outDir": "dist",
    "baseUrl": ".",
    "paths": {
      "@/*": ["src/*"]
    },
    "types": ["node", "vite/client"]
  },
  "include": ["src/**/*.ts", "src/**/*.tsx"],
  "exclude": ["node_modules", "dist", "public/assets"]
}
`,
  '/var/www/api/docker-compose.yml': `# Local and staging stack for the Acme API.
name: acme

x-app-env: &app-env
  APP_ENV: \${APP_ENV:-local}
  DB_HOST: db
  REDIS_HOST: redis
  LOG_CHANNEL: stderr

services:
  app:
    build:
      context: .
      dockerfile: Dockerfile
      target: runtime
      args:
        PHP_VERSION: "8.3"
    image: registry.example.test/acme/api:\${TAG:-latest}
    restart: unless-stopped
    environment:
      <<: *app-env
    env_file: .env
    volumes:
      - ./storage:/var/www/api/storage
    depends_on:
      db:
        condition: service_healthy
      redis:
        condition: service_started
    networks: [backend]

  nginx:
    image: nginx:1.27-alpine
    ports:
      - "80:80"
      - "443:443"
    volumes:
      - ./public:/var/www/api/public:ro
      - ./docker/nginx/default.conf:/etc/nginx/conf.d/default.conf:ro
    depends_on: [app]
    networks: [backend, frontend]

  db:
    image: mysql:8.4
    command: ["--character-set-server=utf8mb4", "--max-connections=200"]
    environment:
      MYSQL_DATABASE: acme
      MYSQL_USER: acme_app
      MYSQL_PASSWORD: change-me
      MYSQL_ROOT_PASSWORD: change-me-too
    volumes:
      - db-data:/var/lib/mysql
      - ./db/schema.sql:/docker-entrypoint-initdb.d/001-schema.sql:ro
    healthcheck:
      test: ["CMD", "mysqladmin", "ping", "-h", "127.0.0.1"]
      interval: 10s
      timeout: 5s
      retries: 6
    networks: [backend]

  redis:
    image: redis:7.4-alpine
    command: redis-server --appendonly yes --maxmemory 256mb --maxmemory-policy allkeys-lru
    volumes:
      - redis-data:/data
    networks: [backend]

volumes:
  db-data:
  redis-data:

networks:
  backend:
    internal: true
  frontend:
`,
  '/var/www/api/Dockerfile': `# syntax=docker/dockerfile:1.7
ARG PHP_VERSION=8.3
ARG COMPOSER_VERSION=2.8

# ---------- dependencies ----------
FROM composer:\${COMPOSER_VERSION} AS vendor
WORKDIR /app
COPY composer.json composer.lock ./
RUN composer install \\
      --no-dev \\
      --no-scripts \\
      --no-interaction \\
      --prefer-dist \\
      --optimize-autoloader

# ---------- front-end assets ----------
FROM node:20-alpine AS assets
WORKDIR /app
COPY package.json pnpm-lock.yaml ./
RUN corepack enable && pnpm install --frozen-lockfile
COPY . .
RUN pnpm run build

# ---------- runtime ----------
FROM php:\${PHP_VERSION}-fpm-alpine AS runtime
LABEL org.opencontainers.image.source="https://git.example.test/acme/api" \\
      org.opencontainers.image.vendor="Acme"

ENV APP_ENV=production \\
    PHP_OPCACHE_VALIDATE_TIMESTAMPS=0 \\
    PHP_MEMORY_LIMIT=256M

RUN apk add --no-cache icu-libs libzip fcgi \\
 && apk add --no-cache --virtual .build-deps $PHPIZE_DEPS icu-dev libzip-dev \\
 && docker-php-ext-install -j"$(nproc)" intl opcache pdo_mysql zip \\
 && pecl install redis-6.1.0 && docker-php-ext-enable redis \\
 && apk del .build-deps

COPY docker/php/opcache.ini /usr/local/etc/php/conf.d/opcache.ini
COPY docker/php/www.conf /usr/local/etc/php-fpm.d/www.conf

WORKDIR /var/www/api
COPY --chown=www-data:www-data . .
COPY --from=vendor --chown=www-data:www-data /app/vendor ./vendor
COPY --from=assets /app/public/build ./public/build

RUN php artisan config:cache && php artisan route:cache \\
 && mkdir -p storage/logs && chown -R www-data:www-data storage bootstrap/cache

USER www-data
EXPOSE 9000

HEALTHCHECK --interval=30s --timeout=3s --start-period=20s --retries=3 \\
  CMD SCRIPT_NAME=/ping SCRIPT_FILENAME=/ping REQUEST_METHOD=GET cgi-fcgi -bind -connect 127.0.0.1:9000 || exit 1

COPY docker/entrypoint.sh /usr/local/bin/entrypoint
ENTRYPOINT ["/usr/local/bin/entrypoint"]
CMD ["php-fpm", "--nodaemonize"]
`,
  '/var/www/api/.env': `# Production environment -- managed by deploy.sh, do not edit by hand.
APP_NAME="Acme API"
APP_ENV=production
APP_KEY=base64:Zm9vYmFyYmF6cXV4Y2hhbmdlbWVjaGFuZ2VtZQ==
APP_DEBUG=false
APP_URL=https://api.example.com

LOG_CHANNEL=daily
LOG_LEVEL=info

# Primary database (private network)
DB_CONNECTION=mysql
DB_HOST=10.0.0.21
DB_PORT=3306
DB_DATABASE=acme
DB_USERNAME=acme_app
DB_PASSWORD="change-me"

CACHE_STORE=redis
SESSION_DRIVER=redis
SESSION_LIFETIME=120
QUEUE_CONNECTION=redis

REDIS_HOST=10.0.0.31
REDIS_PASSWORD=null
REDIS_PORT=6379

MAIL_MAILER=smtp
MAIL_HOST=smtp.example.com
MAIL_PORT=587
MAIL_USERNAME=no-reply@example.com
MAIL_PASSWORD='change-me'
MAIL_FROM_ADDRESS="no-reply@example.com"
MAIL_FROM_NAME="\${APP_NAME}"

AWS_ACCESS_KEY_ID=AKIAEXAMPLEFAKEKEY00
AWS_SECRET_ACCESS_KEY=fake/secret/value/for/demo/only/change-me
AWS_DEFAULT_REGION=eu-west-1
AWS_BUCKET=acme-uploads-example

FEATURE_NEW_BILLING=false
`,
  '/var/www/api/.env.example': `# Copy to .env and fill in the blanks:  cp .env.example .env
# Then run:  php artisan key:generate

APP_NAME="Acme API"
APP_ENV=local
APP_KEY=
APP_DEBUG=true
APP_URL=http://localhost:8080

# debug | info | notice | warning | error
LOG_CHANNEL=stack
LOG_LEVEL=debug

DB_CONNECTION=mysql
DB_HOST=127.0.0.1
DB_PORT=3306
DB_DATABASE=acme
DB_USERNAME=acme_app
DB_PASSWORD="change-me"

CACHE_STORE=file
SESSION_DRIVER=file
SESSION_LIFETIME=120
QUEUE_CONNECTION=sync

REDIS_HOST=127.0.0.1
REDIS_PASSWORD=null
REDIS_PORT=6379

# Mailpit catches everything locally: http://localhost:8025
MAIL_MAILER=smtp
MAIL_HOST=127.0.0.1
MAIL_PORT=1025
MAIL_FROM_ADDRESS="dev@example.test"
MAIL_FROM_NAME="\${APP_NAME}"

AWS_ACCESS_KEY_ID=
AWS_SECRET_ACCESS_KEY=
AWS_DEFAULT_REGION=eu-west-1
AWS_BUCKET=

FEATURE_NEW_BILLING=true
`,
  '/var/www/api/deploy.sh': `#!/usr/bin/env bash
#
# deploy.sh -- zero-downtime release of the Acme API.
#
# Usage: ./deploy.sh [deploy|rollback|status] [--env staging|production]

set -euo pipefail
IFS=$'\\n\\t'

readonly APP_DIR="/var/www/api"
readonly RELEASES_DIR="\${APP_DIR}/releases"
readonly KEEP_RELEASES=5
readonly HOSTS=("10.0.0.11" "10.0.0.12" "10.0.0.13")

ENVIRONMENT="production"
RELEASE="$(date -u +%Y%m%d%H%M%S)"

log()  { printf '\\033[1;34m[%s]\\033[0m %s\\n' "$(date +%T)" "$*"; }
warn() { printf '\\033[1;33m[warn]\\033[0m %s\\n' "$*" >&2; }
die()  { printf '\\033[1;31m[fail]\\033[0m %s\\n' "$*" >&2; exit 1; }

cleanup() {
  local code=$?
  if (( code != 0 )); then
    warn "deploy aborted with exit code \${code}"
    rm -f "\${APP_DIR}/storage/framework/down"
  fi
}
trap cleanup EXIT

require() {
  local bin
  for bin in "$@"; do
    command -v "$bin" >/dev/null 2>&1 || die "missing dependency: $bin"
  done
}

build_release() {
  local target="\${RELEASES_DIR}/\${RELEASE}"
  log "building release \${RELEASE} for \${ENVIRONMENT}"
  mkdir -p "$target"
  git archive --format=tar HEAD | tar -x -C "$target"
  composer install --working-dir="$target" --no-dev --optimize-autoloader --quiet
  ln -sfn "\${APP_DIR}/shared/.env" "\${target}/.env"
}

activate() {
  local host
  for host in "\${HOSTS[@]}"; do
    log "activating on \${host}"
    ssh -o BatchMode=yes "deploy@\${host}" bash -s <<EOF
set -e
cd \${APP_DIR}
echo '{"retry": 30}' > storage/framework/down
ln -sfn \${RELEASES_DIR}/\${RELEASE} current
php current/artisan migrate --force --no-interaction
sudo systemctl reload php8.3-fpm
rm -f storage/framework/down
EOF
  done
}

prune() {
  local -a old
  mapfile -t old < <(ls -1dt "\${RELEASES_DIR}"/* | tail -n +$((KEEP_RELEASES + 1)))
  if (( \${#old[@]} > 0 )); then
    log "pruning \${#old[@]} old release(s)"
    rm -rf -- "\${old[@]}"
  fi
}

main() {
  local action="\${1:-deploy}"
  shift || true
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --env) ENVIRONMENT="$2"; shift 2 ;;
      -h|--help) sed -n '3,6p' "$0"; exit 0 ;;
      *) die "unknown option: $1" ;;
    esac
  done

  require git composer ssh
  case "$action" in
    deploy)   build_release && activate && prune ;;
    rollback) ls -1dt "\${RELEASES_DIR}"/* | sed -n 2p ;;
    status)   readlink -f "\${APP_DIR}/current" ;;
    *)        die "unknown action: \${action}" ;;
  esac
  log "done in \${SECONDS}s"
}

main "$@"
`,
  '/var/www/api/Makefile': `# Developer shortcuts for the Acme API.
SHELL       := /bin/bash
PHP         ?= php
COMPOSER    ?= composer
DOCKER      ?= docker compose
TAG         ?= $(shell git rev-parse --short HEAD)
REGISTRY    := registry.example.test/acme
IMAGE       := $(REGISTRY)/api:$(TAG)
PHPUNIT_ARGS ?=

.DEFAULT_GOAL := help
.PHONY: help install up down test lint analyse build push migrate seed clean

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) | \\
		awk 'BEGIN {FS = ":.*?## "}; {printf "  \\033[36m%-12s\\033[0m %s\\n", $$1, $$2}'

install: vendor node_modules ## Install PHP and JS dependencies

vendor: composer.json composer.lock
	$(COMPOSER) install --no-interaction
	@touch $@

node_modules: package.json
	pnpm install
	@touch $@

up: ## Start the local stack
	$(DOCKER) up -d --build

down: ## Stop the local stack
	$(DOCKER) down --remove-orphans

test: vendor ## Run the test suite
	$(PHP) vendor/bin/phpunit --colors=always $(PHPUNIT_ARGS)

lint: vendor
	$(PHP) vendor/bin/pint --test

analyse: vendor
	$(PHP) -d memory_limit=1G vendor/bin/phpstan analyse

build: ## Build the production image
	docker build --target runtime -t $(IMAGE) .

push: build
	docker push $(IMAGE)

migrate:
	$(DOCKER) exec app php artisan migrate --force

seed: migrate
	$(DOCKER) exec app php artisan db:seed

clean:
	rm -rf vendor node_modules public/build storage/framework/cache/*
`,
  '/var/www/api/README.md': `# Acme API

The REST backend behind the Acme dashboard: users, teams, billing and exports.
It is a Laravel 11 application with a small Go sidecar for health checks and metrics.

[![build](https://ci.example.test/acme/api/badge.svg)](https://ci.example.test/acme/api)
![PHP](https://img.shields.example.test/badge/php-8.3-blue)

## Contents

- [Requirements](#requirements)
- [Getting started](#getting-started)
- [Configuration](#configuration)
- [Deployment](#deployment)
- [API overview](#api-overview)
- [Troubleshooting](#troubleshooting)

## Requirements

- PHP **8.3** with \`intl\`, \`pdo_mysql\`, \`redis\`
- Composer 2.8+
- Node.js 20 and pnpm
- MySQL 8.4 and Redis 7
- Go 1.23 (only for the sidecar)

## Getting started

\`\`\`bash
git clone https://git.example.test/acme/api.git
cd api
cp .env.example .env
make install
php artisan key:generate
make up migrate seed
\`\`\`

The API is now available at <http://localhost:8080>, and the admin UI at
<http://localhost:5173>.

> **Note:** the seeders create an admin user \`admin@example.test\`. Change its
> password before exposing the environment to anyone.

### Running tests

\`\`\`bash
make test                      # full suite
make test PHPUNIT_ARGS="--filter UserControllerTest"
\`\`\`

## Configuration

Most settings live in \`.env\`; structured defaults are in \`config.yaml\`.

| Variable            | Default     | Description                              |
| ------------------- | ----------- | ---------------------------------------- |
| \`APP_ENV\`           | \`local\`     | \`local\`, \`staging\` or \`production\`       |
| \`APP_DEBUG\`         | \`true\`      | Show stack traces in responses           |
| \`DB_HOST\`           | \`127.0.0.1\` | Primary MySQL host                       |
| \`CACHE_STORE\`       | \`file\`      | \`file\`, \`redis\` or \`array\`               |
| \`QUEUE_CONNECTION\`  | \`sync\`      | Use \`redis\` anywhere but your laptop     |
| \`UPLOAD_MAX_MB\`     | \`64\`        | Maximum upload size, in megabytes        |

Feature flags are read from \`features.*\` in \`config.yaml\` and can be
overridden with \`FEATURE_<NAME>=true\`.

## Deployment

Releases are cut from \`main\` and deployed with \`deploy.sh\`:

1. CI builds and pushes \`registry.example.test/acme/api:<sha>\`.
2. \`./deploy.sh deploy --env staging\` rolls it out to staging.
3. After smoke tests pass, run the same command with \`--env production\`.
4. If anything goes wrong: \`./deploy.sh rollback\`.

Each host keeps the last five releases under \`releases/\`; \`current\` is a
symlink to the active one.

## API overview

All endpoints are prefixed with \`/api/v1\` and require a bearer token.

\`\`\`http
GET /api/v1/users?role=admin&per_page=50 HTTP/1.1
Host: api.example.com
Authorization: Bearer <token>
Accept: application/json
\`\`\`

\`\`\`json
{
  "data": [{ "id": 42, "name": "Ada Example", "role": "admin" }],
  "meta": { "current_page": 1, "per_page": 50, "total": 1 }
}
\`\`\`

Common responses:

- \`200 OK\` -- success
- \`201 Created\` -- resource created, see the \`Location\` header
- \`409 Conflict\` -- e.g. deleting the last administrator
- \`422 Unprocessable Entity\` -- validation failed
- \`429 Too Many Requests\` -- slow down; see \`Retry-After\`

## Troubleshooting

**Migrations hang** -- another deploy probably holds the migration lock. Check
\`SELECT * FROM schema_migrations_lock;\` and wait for it, or release it manually.

**\`Class "Redis" not found\`** -- the \`redis\` PHP extension is missing:
\`pecl install redis && docker-php-ext-enable redis\`.

**Uploads fail above 2 MB** -- raise \`upload_max_filesize\` and
\`post_max_size\` in \`php.ini\`, and \`client_max_body_size\` in nginx.

---

Maintained by the platform team. Questions go to \`#acme-api\` or
[platform@example.com](mailto:platform@example.com).
`,
  '/var/www/api/src/server.ts': `import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import { URL } from 'node:url';

export enum HttpStatus {
  Ok = 200,
  Created = 201,
  NotFound = 404,
  TooManyRequests = 429,
  InternalError = 500,
}

export interface User {
  id: number;
  name: string;
  email: string;
  role: 'admin' | 'editor' | 'viewer';
  createdAt: Date;
}

export interface Page<T> {
  data: T[];
  page: number;
  perPage: number;
  total: number;
}

type Handler = (req: IncomingMessage, params: Record<string, string>) => Promise<[HttpStatus, unknown]>;

const users: User[] = [
  { id: 1, name: 'Ada Example', email: 'ada@example.com', role: 'admin', createdAt: new Date('2026-01-12') },
  { id: 2, name: 'Linus Sample', email: 'linus@example.com', role: 'editor', createdAt: new Date('2026-03-02') },
  { id: 42, name: 'Grace Test', email: 'grace@example.test', role: 'viewer', createdAt: new Date('2026-10-30') },
];

function paginate<T>(items: readonly T[], page = 1, perPage = 25): Page<T> {
  const start = (page - 1) * perPage;
  return { data: items.slice(start, start + perPage), page, perPage, total: items.length };
}

const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

class RateLimiter {
  private readonly hits = new Map<string, number[]>();

  constructor(private readonly limit: number, private readonly windowMs = 60_000) {}

  allow(key: string): boolean {
    const now = Date.now();
    const recent = (this.hits.get(key) ?? []).filter((t) => now - t < this.windowMs);
    recent.push(now);
    this.hits.set(key, recent);
    return recent.length <= this.limit;
  }
}

const routes: Array<[RegExp, Handler]> = [
  [/^\\/api\\/users$/, async (req) => {
    const url = new URL(req.url ?? '/', 'http://localhost');
    const page = Number(url.searchParams.get('page') ?? 1);
    return [HttpStatus.Ok, paginate(users, page)];
  }],
  [/^\\/api\\/users\\/(?<id>\\d+)$/, async (_req, { id }) => {
    await sleep(5); // simulate a database round-trip
    const user = users.find((u) => u.id === Number(id));
    return user ? [HttpStatus.Ok, user] : [HttpStatus.NotFound, { message: \`User \${id} not found\` }];
  }],
];

const limiter = new RateLimiter(120);

async function handle(req: IncomingMessage, res: ServerResponse): Promise<void> {
  const started = performance.now();
  let status: HttpStatus = HttpStatus.NotFound;
  let body: unknown = { message: 'Not found' };

  try {
    if (!limiter.allow(req.socket.remoteAddress ?? 'unknown')) {
      status = HttpStatus.TooManyRequests;
      body = { message: 'Slow down' };
    } else {
      const path = new URL(req.url ?? '/', 'http://localhost').pathname;
      for (const [pattern, handler] of routes) {
        const match = pattern.exec(path);
        if (match) {
          [status, body] = await handler(req, match.groups ?? {});
          break;
        }
      }
    }
  } catch (err) {
    console.error('unhandled error', err);
    status = HttpStatus.InternalError;
    body = { message: err instanceof Error ? err.message : String(err) };
  }

  res.writeHead(status, { 'Content-Type': 'application/json' });
  res.end(JSON.stringify(body));
  console.log(\`\${req.method} \${req.url} \${status} \${(performance.now() - started).toFixed(1)}ms\`);
}

const port = Number(process.env.PORT ?? 8080);
createServer((req, res) => void handle(req, res)).listen(port, '0.0.0.0', () => {
  console.log(\`listening on http://0.0.0.0:\${port}\`);
});
`,
  '/var/www/api/src/App.tsx': `import { useCallback, useEffect, useMemo, useState } from 'react';
import type { User } from './server';

interface UserTableProps {
  users: User[];
  onSelect: (user: User) => void;
  selectedId?: number;
}

function UserTable({ users, onSelect, selectedId }: UserTableProps) {
  if (users.length === 0) {
    return <p className="empty">No users match this filter.</p>;
  }
  return (
    <table className="users">
      <thead>
        <tr>
          <th>Name</th>
          <th>Email</th>
          <th>Role</th>
        </tr>
      </thead>
      <tbody>
        {users.map((user) => (
          <tr
            key={user.id}
            className={user.id === selectedId ? 'selected' : undefined}
            onClick={() => onSelect(user)}
          >
            <td>{user.name}</td>
            <td>
              <a href={\`mailto:\${user.email}\`}>{user.email}</a>
            </td>
            <td>
              <span className={\`badge badge-\${user.role}\`}>{user.role}</span>
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

export default function App() {
  const [users, setUsers] = useState<User[]>([]);
  const [query, setQuery] = useState('');
  const [selected, setSelected] = useState<User | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    const controller = new AbortController();
    fetch('/api/users?per_page=100', { signal: controller.signal })
      .then((res) => {
        if (!res.ok) throw new Error(\`HTTP \${res.status}\`);
        return res.json() as Promise<{ data: User[] }>;
      })
      .then(({ data }) => setUsers(data))
      .catch((err: unknown) => {
        if ((err as Error).name !== 'AbortError') setError(String(err));
      })
      .finally(() => setLoading(false));
    return () => controller.abort();
  }, []);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return q ? users.filter((u) => \`\${u.name} \${u.email}\`.toLowerCase().includes(q)) : users;
  }, [users, query]);

  const handleSelect = useCallback((user: User) => setSelected(user), []);

  return (
    <main className="app">
      <header>
        <h1>Users</h1>
        <input
          type="search"
          placeholder="Filter by name or email..."
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          aria-label="Filter users"
        />
      </header>
      {loading && <div className="spinner" role="status" />}
      {error && <div className="alert alert-error">Failed to load users: {error}</div>}
      {!loading && !error && (
        <UserTable users={filtered} onSelect={handleSelect} selectedId={selected?.id} />
      )}
      {selected && (
        <aside className="details">
          <h2>{selected.name}</h2>
          <p>Member since {new Date(selected.createdAt).toLocaleDateString()}</p>
          <button type="button" onClick={() => setSelected(null)}>
            Close
          </button>
        </aside>
      )}
    </main>
  );
}
`,
  '/var/www/api/src/util.js': `'use strict';

/**
 * Small helpers shared by the dashboard and the legacy pages.
 */

const UNITS = ['B', 'KB', 'MB', 'GB', 'TB'];

/**
 * Format a byte count for humans: 1536 -> "1.5 KB".
 * @param {number} bytes
 * @param {number} [decimals=1]
 * @returns {string}
 */
function formatBytes(bytes, decimals = 1) {
  if (!Number.isFinite(bytes) || bytes < 0) return '-';
  let i = 0;
  let value = bytes;
  while (value >= 1024 && i < UNITS.length - 1) {
    value /= 1024;
    i++;
  }
  return \`\${i === 0 ? value : value.toFixed(decimals)} \${UNITS[i]}\`;
}

/** Delay calls to \`fn\` until \`wait\` ms have passed without a new call. */
function debounce(fn, wait = 250) {
  let timer = null;
  return function debounced(...args) {
    clearTimeout(timer);
    timer = setTimeout(() => fn.apply(this, args), wait);
  };
}

/** Retry an async function with exponential backoff. */
async function retry(task, { attempts = 3, baseDelay = 200 } = {}) {
  let lastError;
  for (let n = 0; n < attempts; n++) {
    try {
      return await task(n);
    } catch (err) {
      lastError = err;
      const delay = baseDelay * 2 ** n + Math.floor(Math.random() * 50);
      await new Promise((resolve) => setTimeout(resolve, delay));
    }
  }
  throw lastError;
}

const slugify = (text) =>
  String(text)
    .normalize('NFKD')
    .replace(/[\\u0300-\\u036f]/g, '')
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '');

function groupBy(items, key) {
  return items.reduce((acc, item) => {
    const k = typeof key === 'function' ? key(item) : item[key];
    (acc[k] ||= []).push(item);
    return acc;
  }, {});
}

module.exports = { formatBytes, debounce, retry, slugify, groupBy };
`,
  '/var/www/api/scripts/migrate': `#!/usr/bin/env python3
"""Apply pending SQL migrations from db/migrations in filename order.

Usage:
    scripts/migrate [--dry-run] [--target VERSION]
"""

import argparse
import hashlib
import os
import sys
from pathlib import Path

MIGRATIONS_DIR = Path(__file__).resolve().parent.parent / "db" / "migrations"
TABLE = "schema_migrations"


def connect():
    import pymysql  # imported lazily so --help works without the driver

    return pymysql.connect(
        host=os.environ.get("DB_HOST", "127.0.0.1"),
        port=int(os.environ.get("DB_PORT", "3306")),
        user=os.environ.get("DB_USERNAME", "acme_app"),
        password=os.environ.get("DB_PASSWORD", "change-me"),
        database=os.environ.get("DB_DATABASE", "acme"),
        autocommit=False,
    )


def checksum(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()[:16]


def pending(applied: set[str], target: str | None) -> list[Path]:
    files = sorted(MIGRATIONS_DIR.glob("*.sql"))
    result = [f for f in files if f.stem not in applied]
    if target:
        result = [f for f in result if f.stem <= target]
    return result


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--dry-run", action="store_true", help="print what would run")
    parser.add_argument("--target", help="stop after this migration version")
    args = parser.parse_args(argv)

    conn = connect()
    with conn.cursor() as cur:
        cur.execute(
            f"CREATE TABLE IF NOT EXISTS {TABLE} ("
            " version VARCHAR(64) PRIMARY KEY, checksum CHAR(16) NOT NULL,"
            " applied_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP)"
        )
        cur.execute(f"SELECT version FROM {TABLE}")
        applied = {row[0] for row in cur.fetchall()}

    todo = pending(applied, args.target)
    if not todo:
        print("Nothing to migrate.")
        return 0

    for path in todo:
        print(f"-> {path.name}" + (" (dry run)" if args.dry_run else ""))
        if args.dry_run:
            continue
        try:
            with conn.cursor() as cur:
                for statement in filter(None, (s.strip() for s in path.read_text().split(";"))):
                    cur.execute(statement)
                cur.execute(
                    f"INSERT INTO {TABLE} (version, checksum) VALUES (%s, %s)",
                    (path.stem, checksum(path)),
                )
            conn.commit()
        except Exception as exc:  # noqa: BLE001
            conn.rollback()
            print(f"!! {path.name} failed: {exc}", file=sys.stderr)
            return 1

    print(f"Applied {len(todo)} migration(s).")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
`,
  '/var/www/api/scripts/backup.py': `"""Nightly database and uploads backup for the Acme API."""

from __future__ import annotations

import functools
import gzip
import logging
import shutil
import subprocess
import time
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Callable, Iterator

log = logging.getLogger("backup")


def timed(func: Callable) -> Callable:
    """Log how long the wrapped step took."""

    @functools.wraps(func)
    def wrapper(*args, **kwargs):
        start = time.perf_counter()
        try:
            return func(*args, **kwargs)
        finally:
            log.info("%s took %.2fs", func.__name__, time.perf_counter() - start)

    return wrapper


def retry(times: int = 3, delay: float = 2.0):
    def decorator(func):
        @functools.wraps(func)
        def wrapper(*args, **kwargs):
            for attempt in range(1, times + 1):
                try:
                    return func(*args, **kwargs)
                except subprocess.CalledProcessError as exc:
                    log.warning("attempt %d/%d failed: %s", attempt, times, exc)
                    if attempt == times:
                        raise
                    time.sleep(delay * attempt)
        return wrapper
    return decorator


@dataclass
class BackupConfig:
    database: str = "acme"
    host: str = "10.0.0.21"
    target_dir: Path = Path("/var/backups/acme")
    retention_days: int = 21
    include: list[str] = field(default_factory=lambda: ["storage/app/uploads"])


class BackupJob:
    def __init__(self, config: BackupConfig) -> None:
        self.config = config
        self.stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")

    @property
    def dump_path(self) -> Path:
        return self.config.target_dir / f"{self.config.database}-{self.stamp}.sql.gz"

    @timed
    @retry(times=3)
    def dump_database(self) -> Path:
        self.config.target_dir.mkdir(parents=True, exist_ok=True)
        cmd = ["mysqldump", "--single-transaction", "-h", self.config.host, self.config.database]
        with subprocess.Popen(cmd, stdout=subprocess.PIPE) as proc, gzip.open(self.dump_path, "wb") as out:
            assert proc.stdout is not None
            shutil.copyfileobj(proc.stdout, out)
        if proc.returncode:
            raise subprocess.CalledProcessError(proc.returncode, cmd)
        return self.dump_path

    @timed
    def archive_files(self) -> list[Path]:
        archives = []
        for rel in self.config.include:
            base = self.config.target_dir / f"{Path(rel).name}-{self.stamp}"
            archives.append(Path(shutil.make_archive(str(base), "gztar", "/var/www/api", rel)))
        return archives

    def expired(self) -> Iterator[Path]:
        cutoff = datetime.now(timezone.utc) - timedelta(days=self.config.retention_days)
        for path in self.config.target_dir.glob("*.gz"):
            if datetime.fromtimestamp(path.stat().st_mtime, timezone.utc) < cutoff:
                yield path

    def run(self) -> None:
        dump = self.dump_database()
        files = self.archive_files()
        removed = [p.unlink() or p for p in self.expired()]
        size_mb = dump.stat().st_size / 1_048_576
        log.info(f"backup ok: {dump.name} ({size_mb:.1f} MB), {len(files)} archive(s), {len(removed)} pruned")


if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
    BackupJob(BackupConfig()).run()
`,
  '/var/www/api/scripts/rotate': `#!/bin/sh
# rotate -- compress and prune application logs (POSIX sh, run from cron).
#
#   15 0 * * *  /var/www/api/scripts/rotate /var/www/api/storage/logs 14

set -eu

LOG_DIR=\${1:-/var/www/api/storage/logs}
KEEP_DAYS=\${2:-14}
STAMP=$(date +%Y-%m-%d)

usage() {
    echo "usage: $0 [log_dir] [keep_days]" >&2
    exit 2
}

[ -d "$LOG_DIR" ] || usage

case $KEEP_DAYS in
    ''|*[!0-9]*) echo "keep_days must be a number" >&2; usage ;;
esac

rotated=0
for file in "$LOG_DIR"/*.log; do
    [ -f "$file" ] || continue
    size=$(wc -c < "$file")
    if [ "$size" -eq 0 ]; then
        continue
    fi
    target="$file.$STAMP"
    if [ -e "$target.gz" ]; then
        target="$target.$$"
    fi
    cp "$file" "$target" && : > "$file"
    gzip -9 "$target"
    rotated=$((rotated + 1))
done

# Drop archives older than KEEP_DAYS.
find "$LOG_DIR" -name '*.log.*.gz' -type f -mtime +"$KEEP_DAYS" -exec rm -f {} \\;

if command -v logger >/dev/null 2>&1; then
    logger -t rotate "rotated $rotated file(s) in $LOG_DIR, kept $KEEP_DAYS days"
fi

exit 0
`,
  '/var/www/api/db/schema.sql': `-- Acme API schema (MySQL 8.4)
-- Generated from migrations; keep in sync with db/migrations.

SET NAMES utf8mb4;
SET FOREIGN_KEY_CHECKS = 0;

CREATE TABLE IF NOT EXISTS teams (
    id          BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    slug        VARCHAR(64)     NOT NULL,
    name        VARCHAR(120)    NOT NULL,
    plan        ENUM('free', 'pro', 'enterprise') NOT NULL DEFAULT 'free',
    seats       SMALLINT UNSIGNED NOT NULL DEFAULT 5,
    created_at  TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (id),
    UNIQUE KEY uq_teams_slug (slug)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS users (
    id                 BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    team_id            BIGINT UNSIGNED NULL,
    name               VARCHAR(120) NOT NULL,
    email              VARCHAR(255) NOT NULL,
    password           VARCHAR(255) NOT NULL,
    role               VARCHAR(16)  NOT NULL DEFAULT 'viewer',
    timezone           VARCHAR(64)  NOT NULL DEFAULT 'UTC',
    settings           JSON         NULL,
    email_verified_at  TIMESTAMP NULL,
    last_login_at      TIMESTAMP NULL,
    created_at         TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at         TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    deleted_at         TIMESTAMP NULL,
    PRIMARY KEY (id),
    UNIQUE KEY uq_users_email (email),
    KEY idx_users_team (team_id),
    KEY idx_users_role_login (role, last_login_at),
    CONSTRAINT fk_users_team FOREIGN KEY (team_id) REFERENCES teams (id) ON DELETE SET NULL,
    CONSTRAINT chk_users_role CHECK (role IN ('admin', 'editor', 'viewer'))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS invoices (
    id           BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    team_id      BIGINT UNSIGNED NOT NULL,
    number       CHAR(12)       NOT NULL,
    amount_cents INT UNSIGNED   NOT NULL,
    currency     CHAR(3)        NOT NULL DEFAULT 'EUR',
    status       VARCHAR(16)    NOT NULL DEFAULT 'draft',
    issued_on    DATE           NOT NULL,
    paid_at      DATETIME       NULL,
    PRIMARY KEY (id),
    UNIQUE KEY uq_invoices_number (number),
    KEY idx_invoices_team_status (team_id, status),
    CONSTRAINT fk_invoices_team FOREIGN KEY (team_id) REFERENCES teams (id) ON DELETE CASCADE,
    CONSTRAINT chk_invoices_amount CHECK (amount_cents > 0)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS audit_log (
    id          BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    user_id     BIGINT UNSIGNED NULL,
    action      VARCHAR(64)  NOT NULL,
    subject     VARCHAR(255) NULL,
    ip          VARBINARY(16) NULL,
    created_at  TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
    PRIMARY KEY (id),
    KEY idx_audit_user_time (user_id, created_at)
) ENGINE=InnoDB;

CREATE INDEX idx_invoices_issued ON invoices (issued_on DESC);
CREATE FULLTEXT INDEX ft_users_name_email ON users (name, email);

SET FOREIGN_KEY_CHECKS = 1;

/* Monthly revenue per team, paid invoices only. */
CREATE OR REPLACE VIEW v_team_revenue AS
SELECT
    t.id                              AS team_id,
    t.name                            AS team_name,
    DATE_FORMAT(i.issued_on, '%Y-%m') AS month,
    COUNT(*)                          AS invoices,
    SUM(i.amount_cents) / 100.0       AS revenue
FROM teams t
JOIN invoices i ON i.team_id = t.id
WHERE i.status = 'paid'
  AND i.paid_at IS NOT NULL
GROUP BY t.id, t.name, month
HAVING revenue > 0
ORDER BY month DESC, revenue DESC;

-- Seed data for a fresh environment.
INSERT INTO teams (slug, name, plan, seats) VALUES
    ('acme', 'Acme Inc.', 'enterprise', 250),
    ('example', 'Example Ltd.', 'pro', 20),
    ('sandbox', 'Sandbox', 'free', 5);

INSERT INTO users (team_id, name, email, password, role, email_verified_at) VALUES
    (1, 'Ada Example', 'admin@example.test', '$2y$12$changemechangemechangemeOeFakeHashValue000000000000', 'admin', NOW()),
    (2, 'Linus Sample', 'linus@example.test', '$2y$12$changemechangemechangemeOeFakeHashValue000000000001', 'editor', NULL);

UPDATE teams SET seats = seats + 5 WHERE plan = 'pro' AND seats < 25;
`,
  '/var/www/api/public/index.html': `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta name="description" content="Acme API status and documentation">
  <meta name="theme-color" content="#1f6feb">
  <title>Acme API</title>
  <link rel="icon" href="/favicon.ico" sizes="any">
  <link rel="stylesheet" href="/assets/app.css">
  <style>
    .status { display: inline-flex; align-items: center; gap: 0.5rem; }
    .status::before { content: ""; width: 10px; height: 10px; border-radius: 50%; background: #9aa0a6; }
    .status[data-state="up"]::before { background: #2da44e; }
    .status[data-state="down"]::before { background: #cf222e; }
  </style>
</head>
<body>
  <header class="site-header">
    <a href="/" class="brand">Acme <strong>API</strong></a>
    <nav>
      <a href="/docs">Docs</a>
      <a href="/changelog">Changelog</a>
      <a href="https://status.example.com" rel="noopener">Status</a>
    </nav>
  </header>

  <main class="container">
    <h1>Welcome</h1>
    <p>This host serves the Acme REST API. Point your client at <code>/api/v1</code>.</p>

    <section aria-labelledby="health-title">
      <h2 id="health-title">Health</h2>
      <p class="status" id="health" data-state="unknown">Checking&hellip;</p>
      <p><small>Last checked: <time id="checked-at">never</time></small></p>
    </section>

    <form id="search" class="card" action="/docs/search" method="get">
      <label for="q">Search the docs</label>
      <input id="q" name="q" type="search" placeholder="e.g. pagination" required>
      <button type="submit">Search</button>
    </form>
  </main>

  <footer class="site-footer">&copy; 2026 Acme &middot; <a href="mailto:platform@example.com">Contact</a></footer>

  <script>
    (function () {
      const el = document.getElementById('health');
      const at = document.getElementById('checked-at');

      async function check() {
        try {
          const res = await fetch('/healthz', { cache: 'no-store' });
          const body = await res.json();
          el.dataset.state = body.healthy ? 'up' : 'down';
          el.textContent = body.healthy ? 'All systems operational' : 'Degraded: ' + Object.keys(body.checks).join(', ');
        } catch (e) {
          el.dataset.state = 'down';
          el.textContent = 'Unreachable';
        }
        at.textContent = new Date().toLocaleTimeString();
      }

      check();
      setInterval(check, 30 * 1000);
    })();
  </script>
  <script src="/assets/app.js" defer></script>
</body>
</html>
`,
  '/var/www/api/public/.htaccess': `# Apache rules for the public/ document root.
Options -Indexes -MultiViews +FollowSymLinks

<IfModule mod_rewrite.c>
    RewriteEngine On
    RewriteBase /

    # Redirect www to the bare domain
    RewriteCond %{HTTP_HOST} ^www\\.(.+)$ [NC]
    RewriteRule ^ https://%1%{REQUEST_URI} [L,R=301]

    # Force HTTPS (but not for the health probe)
    RewriteCond %{HTTPS} !=on
    RewriteCond %{REQUEST_URI} !^/healthz$
    RewriteRule ^ https://%{HTTP_HOST}%{REQUEST_URI} [L,R=301]

    # Pass the Authorization header to PHP
    RewriteCond %{HTTP:Authorization} .
    RewriteRule .* - [E=HTTP_AUTHORIZATION:%{HTTP:Authorization}]

    # Remove trailing slashes from non-directories
    RewriteCond %{REQUEST_FILENAME} !-d
    RewriteCond %{REQUEST_URI} (.+)/$
    RewriteRule ^ %1 [L,R=301]

    # Front controller
    RewriteCond %{REQUEST_FILENAME} !-f
    RewriteCond %{REQUEST_FILENAME} !-d
    RewriteRule ^ index.php [L]
</IfModule>

<FilesMatch "\\.(env|log|sql|ini|sh)$">
    Require all denied
</FilesMatch>

<IfModule mod_headers.c>
    Header always set X-Content-Type-Options "nosniff"
    Header always set Referrer-Policy "strict-origin-when-cross-origin"
    <FilesMatch "\\.(css|js|svg|woff2)$">
        Header set Cache-Control "public, max-age=31536000, immutable"
    </FilesMatch>
</IfModule>

php_value upload_max_filesize 64M
php_value post_max_size 64M
`,
  '/var/www/api/public/assets/app.css': `/* Acme API -- base stylesheet */
:root {
  --color-bg: #ffffff;
  --color-fg: #1f2328;
  --color-muted: #656d76;
  --color-accent: #1f6feb;
  --color-border: #d0d7de;
  --radius: 6px;
  --space: 1rem;
  --font-sans: system-ui, -apple-system, 'Segoe UI', Roboto, sans-serif;
  --font-mono: ui-monospace, 'SFMono-Regular', Menlo, monospace;
}

*,
*::before,
*::after {
  box-sizing: border-box;
}

html {
  font-size: 16px;
  -webkit-text-size-adjust: 100%;
}

body {
  margin: 0;
  font: 400 1rem/1.5 var(--font-sans);
  color: var(--color-fg);
  background: var(--color-bg);
}

a {
  color: var(--color-accent);
  text-decoration: none;
}
a:hover,
a:focus-visible {
  text-decoration: underline;
}

code {
  font-family: var(--font-mono);
  font-size: 0.875em;
  padding: 0.1em 0.35em;
  border-radius: calc(var(--radius) / 2);
  background: rgba(175, 184, 193, 0.2);
}

.container {
  max-width: 960px;
  margin: 0 auto;
  padding: calc(var(--space) * 2) var(--space);
}

.site-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 0.75rem var(--space);
  border-bottom: 1px solid var(--color-border);
}
.site-header nav a + a {
  margin-left: 1.25rem;
}

.card {
  display: grid;
  grid-template-columns: 1fr auto;
  gap: 0.5rem;
  padding: var(--space);
  border: 1px solid var(--color-border);
  border-radius: var(--radius);
}

button {
  cursor: pointer;
  padding: 0.4rem 1rem;
  border: 0;
  border-radius: var(--radius);
  color: #fff;
  background: var(--color-accent);
  transition: filter 120ms ease-in-out;
}
button:hover {
  filter: brightness(1.1);
}

@media (prefers-color-scheme: dark) {
  :root {
    --color-bg: #0d1117;
    --color-fg: #e6edf3;
    --color-muted: #8d96a0;
    --color-border: #30363d;
  }
}

@media (max-width: 600px) {
  .card {
    grid-template-columns: 1fr;
  }
  .site-header nav {
    display: none;
  }
}
`,
  '/var/www/api/public/assets/theme.scss': `// Dashboard theme, compiled to public/build/theme.css by Vite.
@use 'sass:math';
@use 'sass:color';

$brand: #1f6feb;
$danger: #cf222e;
$success: #2da44e;
$radius: 6px;
$breakpoints: (
  sm: 600px,
  md: 900px,
  lg: 1200px,
);

@mixin respond($size) {
  $width: map-get($breakpoints, $size);
  @media (min-width: $width) {
    @content;
  }
}

@mixin button-variant($bg, $fg: #fff) {
  color: $fg;
  background-color: $bg;
  border: 1px solid color.adjust($bg, $lightness: -8%);

  &:hover:not(:disabled) {
    background-color: color.adjust($bg, $lightness: 6%);
  }
  &:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }
}

%card-base {
  border-radius: $radius;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.12);
}

.btn {
  padding: math.div($radius, 2) $radius * 2;
  border-radius: $radius;
  font-weight: 600;

  &-primary { @include button-variant($brand); }
  &-danger  { @include button-variant($danger); }
  &-success { @include button-variant($success); }
}

.users {
  width: 100%;
  border-collapse: collapse;

  th,
  td {
    padding: 0.5rem 0.75rem;
    text-align: left;
  }

  tr.selected td {
    background: rgba($brand, 0.08);
  }

  .badge {
    @extend %card-base;
    padding: 0 0.4rem;
    font-size: 0.75rem;

    @each $role, $color in (admin: $danger, editor: $brand, viewer: #6e7781) {
      &-#{$role} {
        color: $color;
        border: 1px solid rgba($color, 0.4);
      }
    }
  }
}

.details {
  @extend %card-base;
  padding: 1rem;

  @include respond(md) {
    position: fixed;
    right: 1rem;
    top: 5rem;
    width: 320px;
  }
}
`,
  '/var/www/api/public/assets/app.js': `/* Progressive enhancements for the public pages. */
(function (window, document) {
  'use strict';

  var STORAGE_KEY = 'acme.theme';
  var root = document.documentElement;

  function getTheme() {
    try {
      return window.localStorage.getItem(STORAGE_KEY) || 'auto';
    } catch (e) {
      return 'auto';
    }
  }

  function applyTheme(theme) {
    if (theme === 'auto') {
      root.removeAttribute('data-theme');
    } else {
      root.setAttribute('data-theme', theme);
    }
  }

  function copyCodeBlocks() {
    var blocks = document.querySelectorAll('pre > code');
    Array.prototype.forEach.call(blocks, function (code) {
      var button = document.createElement('button');
      button.type = 'button';
      button.className = 'copy';
      button.textContent = 'Copy';
      button.addEventListener('click', function () {
        navigator.clipboard.writeText(code.innerText).then(function () {
          button.textContent = 'Copied!';
          setTimeout(function () { button.textContent = 'Copy'; }, 1500);
        });
      });
      code.parentNode.insertBefore(button, code);
    });
  }

  function trackOutboundLinks() {
    document.addEventListener('click', function (event) {
      var link = event.target.closest && event.target.closest('a[href^="http"]');
      if (!link || link.host === window.location.host) return;
      if (navigator.sendBeacon) {
        navigator.sendBeacon('/api/v1/events', JSON.stringify({ type: 'outbound', href: link.href }));
      }
    });
  }

  document.addEventListener('DOMContentLoaded', function () {
    applyTheme(getTheme());
    copyCodeBlocks();
    trackOutboundLinks();

    var form = document.getElementById('search');
    if (form) {
      form.addEventListener('submit', function (event) {
        var q = form.elements.q.value.trim();
        if (q.length < 2) {
          event.preventDefault();
          form.elements.q.focus();
        }
      });
    }
  });
})(window, document);
`,
  '/var/www/api/public/sitemap.xml': `<?xml version="1.0" encoding="UTF-8"?>
<!-- Generated by \`php artisan sitemap:generate\` on 2026-11-07 -->
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"
        xmlns:xhtml="http://www.w3.org/1999/xhtml">
  <url>
    <loc>https://api.example.com/</loc>
    <lastmod>2026-11-07</lastmod>
    <changefreq>weekly</changefreq>
    <priority>1.0</priority>
  </url>
  <url>
    <loc>https://api.example.com/docs</loc>
    <lastmod>2026-11-05</lastmod>
    <changefreq>weekly</changefreq>
    <priority>0.9</priority>
    <xhtml:link rel="alternate" hreflang="fr" href="https://api.example.com/fr/docs"/>
    <xhtml:link rel="alternate" hreflang="pt" href="https://api.example.com/pt/docs"/>
  </url>
  <url>
    <loc>https://api.example.com/docs/authentication</loc>
    <lastmod>2026-10-21</lastmod>
    <changefreq>monthly</changefreq>
    <priority>0.8</priority>
  </url>
  <url>
    <loc>https://api.example.com/docs/pagination</loc>
    <lastmod>2026-09-14</lastmod>
    <changefreq>monthly</changefreq>
    <priority>0.7</priority>
  </url>
  <url>
    <loc>https://api.example.com/changelog</loc>
    <lastmod>2026-11-07</lastmod>
    <changefreq>daily</changefreq>
    <priority>0.6</priority>
  </url>
  <url>
    <loc>https://api.example.com/legal/terms?lang=en&amp;v=3</loc>
    <lastmod>2026-01-01</lastmod>
    <changefreq>yearly</changefreq>
    <priority>0.2</priority>
  </url>
</urlset>
`,
  '/var/www/api/storage/logs/laravel.log': `[2026-11-07 10:40:12] production.INFO: Queue worker started {"queue":"default","pid":2214}
[2026-11-07 10:40:12] production.INFO: Queue worker started {"queue":"mail","pid":2215}
[2026-11-07 10:40:58] production.DEBUG: Cache miss {"key":"users:42","store":"redis"}
[2026-11-07 10:41:03] production.INFO: User logged in {"user_id":42,"ip":"203.0.113.57"}
[2026-11-07 10:41:17] production.DEBUG: Query executed {"sql":"select * from \`teams\` where \`id\` = ?","time":1.84}
[2026-11-07 10:41:30] production.WARNING: Slow query detected {"sql":"select * from \`invoices\` where \`team_id\` = ? order by \`issued_on\` desc","time":1287.4}
[2026-11-07 10:41:44] production.INFO: Export queued {"export_id":"exp_7f3a9c01","rows":18240,"user_id":42}
[2026-11-07 10:42:01] production.ERROR: SQLSTATE[HY000] [2002] Connection refused (Connection: mysql, SQL: select * from \`users\` where \`email\` = admin@example.test limit 1) {"exception":"[object] (Illuminate\\\\Database\\\\QueryException(code: 2002): SQLSTATE[HY000] [2002] Connection refused (Connection: mysql, SQL: select * from \`users\` where \`email\` = admin@example.test limit 1) at /var/www/api/vendor/laravel/framework/src/Illuminate/Database/Connection.php:825)
[stacktrace]
#0 /var/www/api/vendor/laravel/framework/src/Illuminate/Database/Connection.php(779): Illuminate\\\\Database\\\\Connection->runQueryCallback()
#1 /var/www/api/vendor/laravel/framework/src/Illuminate/Database/Connection.php(398): Illuminate\\\\Database\\\\Connection->run()
#2 /var/www/api/vendor/laravel/framework/src/Illuminate/Database/Query/Builder.php(3133): Illuminate\\\\Database\\\\Connection->select()
#3 /var/www/api/vendor/laravel/framework/src/Illuminate/Database/Query/Builder.php(3118): Illuminate\\\\Database\\\\Query\\\\Builder->runSelect()
#4 /var/www/api/vendor/laravel/framework/src/Illuminate/Database/Eloquent/Builder.php(739): Illuminate\\\\Database\\\\Query\\\\Builder->get()
#5 /var/www/api/vendor/laravel/framework/src/Illuminate/Database/Eloquent/Builder.php(330): Illuminate\\\\Database\\\\Eloquent\\\\Builder->get()
#6 /var/www/api/app/Http/Controllers/AuthController.php(41): Illuminate\\\\Database\\\\Eloquent\\\\Builder->first()
#7 /var/www/api/vendor/laravel/framework/src/Illuminate/Routing/Controller.php(54): App\\\\Http\\\\Controllers\\\\AuthController->login()
#8 /var/www/api/vendor/laravel/framework/src/Illuminate/Routing/ControllerDispatcher.php(43): Illuminate\\\\Routing\\\\Controller->callAction()
#9 /var/www/api/vendor/laravel/framework/src/Illuminate/Routing/Route.php(260): Illuminate\\\\Routing\\\\ControllerDispatcher->dispatch()
#10 /var/www/api/vendor/laravel/framework/src/Illuminate/Routing/Router.php(808): Illuminate\\\\Routing\\\\Route->run()
#11 /var/www/api/vendor/laravel/framework/src/Illuminate/Pipeline/Pipeline.php(169): Illuminate\\\\Routing\\\\Router->Illuminate\\\\Routing\\\\{closure}()
#12 /var/www/api/app/Http/Middleware/AssignRequestId.php(22): Illuminate\\\\Pipeline\\\\Pipeline->Illuminate\\\\Pipeline\\\\{closure}()
#13 /var/www/api/vendor/laravel/framework/src/Illuminate/Foundation/Http/Kernel.php(175): Illuminate\\\\Pipeline\\\\Pipeline->then()
#14 /var/www/api/index.php(54): Illuminate\\\\Foundation\\\\Http\\\\Kernel->handle()
#15 {main}

[previous exception] [object] (PDOException(code: 2002): SQLSTATE[HY000] [2002] Connection refused at /var/www/api/vendor/laravel/framework/src/Illuminate/Database/Connectors/Connector.php:66)
[stacktrace]
#0 /var/www/api/vendor/laravel/framework/src/Illuminate/Database/Connectors/Connector.php(66): PDO->__construct()
#1 /var/www/api/vendor/laravel/framework/src/Illuminate/Database/Connectors/MySqlConnector.php(24): Illuminate\\\\Database\\\\Connectors\\\\Connector->createConnection()
#2 {main}
"}
[2026-11-07 10:42:02] production.WARNING: Retrying database connection {"attempt":1,"host":"10.0.0.21"}
[2026-11-07 10:42:04] production.WARNING: Retrying database connection {"attempt":2,"host":"10.0.0.21"}
[2026-11-07 10:42:08] production.INFO: Database connection restored {"host":"10.0.0.21","after_ms":6412}
[2026-11-07 10:42:15] production.DEBUG: Cache hit {"key":"users:42","store":"redis"}
[2026-11-07 10:42:31] production.INFO: Invoice paid {"invoice":"INV-2026-0981","team_id":2,"amount_cents":4900}
[2026-11-07 10:43:02] production.NOTICE: Rate limit reached {"ip":"203.0.113.77","route":"api/v1/users","limit":120}
[2026-11-07 10:43:40] production.INFO: Export finished {"export_id":"exp_7f3a9c01","duration_s":115.2,"size":"4.1 MB"}
[2026-11-07 10:44:05] production.DEBUG: Mail queued {"to":"linus@example.test","mailable":"App\\\\Mail\\\\ExportReady"}
[2026-11-07 10:44:06] production.INFO: Mail sent {"to":"linus@example.test","transport":"smtp"}
[2026-11-07 10:45:19] production.WARNING: Deprecated: Creation of dynamic property App\\\\Support\\\\Legacy\\\\Report::$total is deprecated in /var/www/api/app/Support/Legacy/Report.php on line 88
[2026-11-07 10:46:00] production.INFO: Scheduler ran {"task":"invoices:remind","durationMs":312}
[2026-11-07 10:47:12] production.ERROR: Call to a member function format() on null {"userId":17,"exception":"[object] (Error(code: 0): Call to a member function format() on null at /var/www/api/app/Http/Resources/UserResource.php:31)
[stacktrace]
#0 /var/www/api/vendor/laravel/framework/src/Illuminate/Http/Resources/Json/JsonResource.php(107): App\\\\Http\\\\Resources\\\\UserResource->toArray()
#1 /var/www/api/vendor/laravel/framework/src/Illuminate/Http/Resources/Json/ResourceResponse.php(39): Illuminate\\\\Http\\\\Resources\\\\Json\\\\JsonResource->resolve()
#2 /var/www/api/app/Http/Controllers/UserController.php(48): Illuminate\\\\Http\\\\Resources\\\\Json\\\\JsonResource->toResponse()
#3 {main}
"}
[2026-11-07 10:47:13] production.INFO: Request finished {"method":"GET","uri":"/api/v1/users/17","status":500,"ms":38}
[2026-11-07 10:48:44] production.DEBUG: Feature flag evaluated {"flag":"new_billing","team_id":1,"result":false}
[2026-11-07 10:49:30] production.INFO: User logged out {"user_id":42}
[2026-11-07 10:50:00] production.INFO: Scheduler ran {"task":"backups:prune","durationMs":1204}
[2026-11-07 10:51:27] production.CRITICAL: Disk usage above threshold {"mount":"/var/www/api/storage","used_pct":93}
[2026-11-07 10:52:03] production.INFO: Old exports deleted {"count":38,"freed":"1.2 GB"}
`,
  '/var/www/api/app.ini': `; Acme legacy reporting module configuration
; Lines starting with ';' or '#' are comments.

[general]
name = "Acme Reports"
version = 2.4.1
debug = false
timezone = Europe/Lisbon
locale = pt_PT

[database]
driver = mysql
host = 10.0.0.21
port = 3306
dbname = acme_reports
user = reports
password = "change-me"
persistent = on

[cache]
enabled = true
backend = redis
ttl = 600
servers[] = "10.0.0.31:6379"
servers[] = "10.0.0.32:6379"

[paths]
base = /var/www/api
exports = \${paths.base}/storage/exports
templates = "\${paths.base}/resources/reports"

[mail]
smtp_host = smtp.example.com
smtp_port = 587
from = "reports@example.com"

# Per-report overrides
[report:monthly]
schedule = "0 6 1 * *"
recipients = finance@example.com, ceo@example.com
format = pdf

[report:weekly]
schedule = "0 7 * * 1"
format = csv
include_archived = no
`,
  '/var/www/api/settings.properties': `# Search indexer settings (Java)
# Generated 2026-11-07, edit with care.

app.name=Acme Search Indexer
app.version=3.2.0
app.environment=production

# HTTP endpoint exposed for health checks
server.port=8090
server.address=0.0.0.0
server.shutdown=graceful

# Source database
spring.datasource.url=jdbc:mysql://10.0.0.21:3306/acme?useSSL=true&serverTimezone=UTC
spring.datasource.username=indexer
spring.datasource.password=change-me
spring.datasource.hikari.maximum-pool-size=10
spring.datasource.hikari.connection-timeout=30000

# Search cluster
search.hosts=10.0.0.41:9200,10.0.0.42:9200
search.index.prefix=acme_
search.index.shards=3
search.index.replicas=1
search.bulk.size=500
search.bulk.flush-interval=5s

# Indexing schedule (cron, seconds field first)
indexer.full.cron=0 0 2 * * *
indexer.incremental.cron=0 */5 * * * *
indexer.entities=users,teams,invoices

# Multi-line value with continuation
indexer.analyzer.stopwords=a, an, and, are, as, at, be, but, by, \\
    for, if, in, into, is, it, no, not, of, on, or, \\
    such, that, the, their, then, there, these, they

logging.level.root=INFO
logging.level.com.example.acme=DEBUG
logging.file.name=/var/log/acme/indexer.log
management.endpoints.web.exposure.include=health,info,metrics
`,
  '/var/www/api/pyproject.toml': `[build-system]
requires = ["setuptools>=69", "wheel"]
build-backend = "setuptools.build_meta"

[project]
name = "acme-ops"
version = "0.9.2"
description = "Operational scripts for the Acme API: migrations, backups, log rotation"
readme = "README.md"
requires-python = ">=3.11"
license = { text = "Proprietary" }
authors = [
  { name = "Platform Team", email = "platform@example.com" },
]
classifiers = [
  "Programming Language :: Python :: 3.11",
  "Programming Language :: Python :: 3.12",
  "Private :: Do Not Upload",
]
dependencies = [
  "pymysql>=1.1,<2",
  "boto3~=1.35",
  "click>=8.1",
  "pyyaml>=6.0.1",
]

[project.optional-dependencies]
dev = ["pytest>=8", "pytest-cov", "ruff>=0.7", "mypy>=1.12"]

[project.scripts]
acme-backup = "scripts.backup:main"
acme-migrate = "scripts.migrate:main"

[tool.setuptools.packages.find]
include = ["scripts*"]

[tool.ruff]
line-length = 110
target-version = "py311"

[tool.ruff.lint]
select = ["E", "F", "I", "B", "UP", "SIM"]
ignore = ["E501"]

[tool.mypy]
strict = true
warn_unused_ignores = true
plugins = []

[tool.pytest.ini_options]
minversion = "8.0"
addopts = "-ra --cov=scripts --cov-report=term-missing"
testpaths = ["tests"]
filterwarnings = ["error", "ignore::DeprecationWarning:boto3.*"]

[[tool.acme.targets]]
name = "staging"
hosts = ["10.0.0.11"]
backup = { enabled = true, retention_days = 7 }

[[tool.acme.targets]]
name = "production"
hosts = ["10.0.0.12", "10.0.0.13"]
backup = { enabled = true, retention_days = 21 }
released = 2026-11-07T09:30:00Z
`,
  '/var/www/api/legacy/latin1.txt': `Relatório de manutenção -- servidor antigo (ISO-8859-1)
========================================================

Résumé en français :
Le serveur a été redémarré à 06h30 après la coupure de courant.
La température de la salle était de 27 °C ; le ventilateur n° 3 est à remplacer.
Coût estimé de la réparation : 145 £ (devis reçu de la société Lefèvre & Fils).
Prochaine étape : vérifier les sauvegardes et noter l'heure exacte de la reprise.

Resumo em português:
A migração dos ficheiros ainda não terminou; faltam as pastas de configuração.
O João e a Conceição vão rever as permissões amanhã às 9h.
Atenção: os nomes com acentuação (ç, ã, õ, é) devem continuar legíveis.

Notizen auf Deutsch: Überprüfung der Größe der Logdateien -- schön wäre ein Cronjob.
Fin du rapport. ¿Preguntas? Contactez le support.
`,
  '/var/www/api/legacy/windows.ini': `; Generated by AcmeReports.exe 4.2.0.118
; Do not edit while the application is running

[Application]
Name=Acme Reports
Version=4.2.0.118
InstallDir=C:\\Program Files\\Acme\\Reports
DataDir=C:\\ProgramData\\Acme\\Reports
Language=1033
LastRun=2026-11-06 18:22:41

[Window]
Left=120
Top=80
Width=1280
Height=800
Maximized=0

[Database]
Server=10.0.0.21
Port=3306
Database=acme_reports
UseWindowsAuth=False
Timeout=30

[RecentFiles]
File1=C:\\Users\\deploy\\Documents\\monthly-2026-10.rpt
File2=C:\\Users\\deploy\\Documents\\weekly-45.rpt
File3=\\\\fileserver\\shared\\reports\\q3-summary.rpt
`,
  '/var/www/api/legacy/mixed.txt': `Notes from the old server migration
-----------------------------------
- Copied /var/www/legacy to the new host on Monday.
- Some files came from a Windows share, others from the Mac mini.
- That is why this file has mixed line endings (LF, CRLF and CR).
- Cron jobs still point to the old PHP 7.4 binary; update them.
- The reports module reads app.ini, not .env.
- Ask finance whether the monthly report can move to the new API.
- Disk on the old box is 93% full, clean storage/exports first.
- Keep the old box running until the end of the month.
- DNS for legacy.example.com switches on the 15th.
- Remember to revoke the old deploy key afterwards.
`,
  '/srv/tools/Cargo.toml': `[package]
name = "logscan"
version = "0.4.1"
edition = "2021"
rust-version = "1.78"
description = "Scan and summarise application log files"
authors = ["Platform Team <platform@example.com>"]
license = "MIT OR Apache-2.0"
repository = "https://git.example.test/acme/logscan"
readme = "README.md"
keywords = ["logs", "cli", "ops"]
categories = ["command-line-utilities"]

[[bin]]
name = "logscan"
path = "src/main.rs"

[dependencies]
anyhow = "1.0"
clap = { version = "4.5", features = ["derive", "env"] }
regex = "1.11"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
time = { version = "0.3", default-features = false, features = ["parsing", "formatting"] }

[dev-dependencies]
pretty_assertions = "1.4"
tempfile = "3.13"

[features]
default = ["color"]
color = []

[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 1
strip = true
panic = "abort"
`,
  '/srv/tools/src/main.rs': `//! logscan -- summarise application logs by level.

use std::{collections::BTreeMap, fmt, fs::File, io::{self, BufRead, BufReader}, path::PathBuf, str::FromStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl FromStr for Level {
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_uppercase().as_str() {
            "DEBUG" => Ok(Level::Debug),
            "INFO" => Ok(Level::Info),
            "WARN" | "WARNING" => Ok(Level::Warn),
            "ERROR" | "CRITICAL" => Ok(Level::Error),
            other => Err(ParseError::UnknownLevel(other.to_owned())),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum ParseError {
    UnknownLevel(String),
    Malformed { line: usize },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::UnknownLevel(l) => write!(f, "unknown level \`{l}\`"),
            ParseError::Malformed { line } => write!(f, "malformed entry on line {line}"),
        }
    }
}

impl std::error::Error for ParseError {}

#[derive(Debug)]
pub struct Entry<'a> {
    pub timestamp: &'a str,
    pub level: Level,
    pub component: &'a str,
    pub message: &'a str,
}

pub trait Summary {
    fn record(&mut self, entry: &Entry<'_>);
    fn report(&self) -> String;
}

#[derive(Default)]
pub struct Counts {
    by_level: BTreeMap<Level, u64>,
}

impl Summary for Counts {
    fn record(&mut self, entry: &Entry<'_>) {
        *self.by_level.entry(entry.level).or_insert(0) += 1;
    }

    fn report(&self) -> String {
        self.by_level.iter().map(|(level, n)| format!("{level:?}\\t{n}\\n")).collect()
    }
}

/// Parse \`2026-11-07T10:42:01.123Z INFO  [http] message...\`.
pub fn parse_line(line: &str, number: usize) -> Result<Entry<'_>, ParseError> {
    let mut parts = line.splitn(3, char::is_whitespace);
    let timestamp = parts.next().filter(|s| !s.is_empty()).ok_or(ParseError::Malformed { line: number })?;
    let level: Level = parts.next().ok_or(ParseError::Malformed { line: number })?.parse()?;
    let rest = parts.next().unwrap_or("").trim_start();
    let (component, message) = match rest.strip_prefix('[').and_then(|r| r.split_once(']')) {
        Some((c, m)) => (c, m.trim()),
        None => ("-", rest),
    };
    Ok(Entry { timestamp, level, component, message })
}

macro_rules! bail_usage {
    ($($arg:tt)*) => {{ eprintln!($($arg)*); std::process::exit(2) }};
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path: PathBuf = std::env::args_os().nth(1).map(PathBuf::from).unwrap_or_else(|| bail_usage!("usage: logscan <file>"));

    let reader: Box<dyn BufRead> = match path.to_str() {
        Some("-") => Box::new(io::stdin().lock()),
        _ => Box::new(BufReader::new(File::open(&path)?)),
    };

    let mut counts = Counts::default();
    for (i, line) in reader.lines().enumerate() {
        let line = line?;
        match parse_line(&line, i + 1) {
            Ok(entry) => counts.record(&entry),
            Err(err) => eprintln!("warning: {err}"),
        }
    }

    print!("{}", counts.report());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_standard_line() {
        let e = parse_line("2026-11-07T10:42:01.123Z INFO  [http] GET /api/users/42 200", 1).unwrap();
        assert_eq!(e.level, Level::Info);
        assert_eq!(e.component, "http");
        assert!(e.message.starts_with("GET"));
    }
}
`,
  '/etc/nginx/nginx.conf': `user www-data;
worker_processes auto;
worker_rlimit_nofile 65535;
pid /run/nginx.pid;
error_log /var/log/nginx/error.log warn;
include /etc/nginx/modules-enabled/*.conf;

events {
    worker_connections 4096;
    multi_accept on;
    use epoll;
}

http {
    ##
    # Basic settings
    ##
    sendfile on;
    tcp_nopush on;
    tcp_nodelay on;
    keepalive_timeout 65;
    types_hash_max_size 2048;
    server_tokens off;
    client_max_body_size 64m;

    include /etc/nginx/mime.types;
    default_type application/octet-stream;

    ##
    # SSL settings
    ##
    ssl_protocols TLSv1.2 TLSv1.3;
    ssl_prefer_server_ciphers off;
    ssl_session_cache shared:SSL:10m;
    ssl_session_timeout 1d;

    ##
    # Logging
    ##
    log_format main '$remote_addr - $remote_user [$time_local] "$request" '
                    '$status $body_bytes_sent "$http_referer" '
                    '"$http_user_agent" rt=$request_time uct=$upstream_connect_time';
    access_log /var/log/nginx/access.log main buffer=32k flush=5s;

    ##
    # Gzip
    ##
    gzip on;
    gzip_vary on;
    gzip_proxied any;
    gzip_comp_level 5;
    gzip_min_length 256;
    gzip_types text/plain text/css application/json application/javascript
               text/xml application/xml image/svg+xml;

    map $http_upgrade $connection_upgrade {
        default upgrade;
        ''      close;
    }

    limit_req_zone $binary_remote_addr zone=api:10m rate=20r/s;

    ##
    # Virtual hosts
    ##
    include /etc/nginx/conf.d/*.conf;
    include /etc/nginx/sites-enabled/*;
}
`,
  '/etc/nginx/sites-available/default': `upstream node_app {
    least_conn;
    server 10.0.0.11:3000 max_fails=3 fail_timeout=10s;
    server 10.0.0.12:3000 max_fails=3 fail_timeout=10s;
    keepalive 32;
}

# Redirect all plain HTTP to HTTPS.
server {
    listen 80 default_server;
    listen [::]:80 default_server;
    server_name example.com www.example.com;

    location /.well-known/acme-challenge/ {
        root /var/www/letsencrypt;
    }

    location / {
        return 301 https://example.com$request_uri;
    }
}

server {
    listen 443 ssl;
    listen [::]:443 ssl;
    http2 on;
    server_name example.com;

    ssl_certificate     /etc/ssl/certs/example.com.fullchain.pem;
    ssl_certificate_key /etc/ssl/private/example.com.key;

    root /var/www/api/public;
    index index.php index.html;

    add_header Strict-Transport-Security "max-age=63072000; includeSubDomains" always;
    add_header X-Frame-Options "SAMEORIGIN" always;
    add_header X-Content-Type-Options "nosniff" always;

    location / {
        try_files $uri $uri/ /index.php?$query_string;
    }

    location ~ \\.php$ {
        try_files $uri =404;
        fastcgi_split_path_info ^(.+\\.php)(/.+)$;
        fastcgi_pass unix:/run/php/php8.3-fpm.sock;
        fastcgi_index index.php;
        include fastcgi_params;
        fastcgi_param SCRIPT_FILENAME $realpath_root$fastcgi_script_name;
        fastcgi_read_timeout 60s;
    }

    location /api/ {
        limit_req zone=api burst=40 nodelay;
        try_files $uri /index.php?$query_string;
    }

    location /app/ {
        proxy_pass http://node_app/;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $connection_upgrade;
        proxy_read_timeout 300s;
    }

    location ~* \\.(?:css|js|woff2?|svg|png|jpe?g|webp)$ {
        expires 30d;
        access_log off;
        add_header Cache-Control "public, immutable";
    }

    location ~ /\\.(?!well-known) {
        deny all;
    }

    error_page 502 503 504 /50x.html;
    location = /50x.html {
        root /usr/share/nginx/html;
    }
}
`,
  '/etc/apache2/apache2.conf': `# This is the main Apache server configuration file.
# See /usr/share/doc/apache2/README.Debian for Debian-specific notes.

ServerRoot "/etc/apache2"
ServerName localhost

Mutex file:\${APACHE_LOCK_DIR} default
DefaultRuntimeDir \${APACHE_RUN_DIR}
PidFile \${APACHE_PID_FILE}

Timeout 300
KeepAlive On
MaxKeepAliveRequests 100
KeepAliveTimeout 5

User \${APACHE_RUN_USER}
Group \${APACHE_RUN_GROUP}

HostnameLookups Off
ErrorLog \${APACHE_LOG_DIR}/error.log
LogLevel warn

# Modules and their configuration
IncludeOptional mods-enabled/*.load
IncludeOptional mods-enabled/*.conf

Include ports.conf

<Directory />
	Options FollowSymLinks
	AllowOverride None
	Require all denied
</Directory>

<Directory /usr/share>
	AllowOverride None
	Require all granted
</Directory>

<Directory /var/www/>
	Options Indexes FollowSymLinks
	AllowOverride All
	Require all granted
</Directory>

AccessFileName .htaccess

<FilesMatch "^\\.ht">
	Require all denied
</FilesMatch>

LogFormat "%v:%p %h %l %u %t \\"%r\\" %>s %O \\"%{Referer}i\\" \\"%{User-Agent}i\\"" vhost_combined
LogFormat "%h %l %u %t \\"%r\\" %>s %O \\"%{Referer}i\\" \\"%{User-Agent}i\\"" combined
LogFormat "%h %l %u %t \\"%r\\" %>s %O" common
LogFormat "%{Referer}i -> %U" referer
LogFormat "%{User-agent}i" agent

<IfModule mod_status.c>
	<Location "/server-status">
		SetHandler server-status
		Require ip 127.0.0.1 10.0.0.0/8
	</Location>
</IfModule>

ServerTokens Prod
ServerSignature Off

# Include generic snippets and the virtual host configurations.
IncludeOptional conf-enabled/*.conf
IncludeOptional sites-enabled/*.conf
`,
  '/etc/apache2/sites-available/000-default.conf': `<VirtualHost *:80>
	ServerName example.com
	ServerAlias www.example.com
	ServerAdmin webmaster@example.com

	DocumentRoot /var/www/api/public

	# Send everything to HTTPS except the ACME challenge.
	RewriteEngine On
	RewriteCond %{REQUEST_URI} !^/\\.well-known/acme-challenge/
	RewriteRule ^/?(.*)$ https://example.com/$1 [R=301,L]

	<Directory /var/www/api/public>
		Options -Indexes +FollowSymLinks
		AllowOverride All
		Require all granted
		DirectoryIndex index.php index.html
	</Directory>

	<FilesMatch \\.php$>
		SetHandler "proxy:unix:/run/php/php8.3-fpm.sock|fcgi://localhost"
	</FilesMatch>

	Alias /legacy /var/www/legacy
	<Directory /var/www/legacy>
		Require ip 10.0.0.0/8 203.0.113.0/24
	</Directory>

	ProxyPreserveHost On
	ProxyPass        /app/ http://10.0.0.11:3000/
	ProxyPassReverse /app/ http://10.0.0.11:3000/

	Header always set X-Content-Type-Options "nosniff"

	LogLevel info rewrite:trace2
	ErrorLog \${APACHE_LOG_DIR}/example.com-error.log
	CustomLog \${APACHE_LOG_DIR}/example.com-access.log combined
</VirtualHost>

# vim: syntax=apache ts=4 sw=4 sts=4 sr noet
`,
  '/home/deploy/.bashrc': `# ~/.bashrc: executed by bash(1) for non-login shells.

# If not running interactively, don't do anything
case $- in
    *i*) ;;
      *) return;;
esac

HISTCONTROL=ignoreboth:erasedups
HISTSIZE=10000
HISTFILESIZE=20000
HISTTIMEFORMAT='%F %T  '
shopt -s histappend checkwinsize globstar

export EDITOR=vim
export PAGER=less
export LESS='-R -F -X'
export PATH="$HOME/.local/bin:$HOME/bin:$PATH"
export APP_DIR=/var/www/api

# Colourful prompt with the current git branch, if any.
__git_branch() {
    git symbolic-ref --short HEAD 2>/dev/null | sed 's/.*/ (&)/'
}
if [ "$(id -u)" -eq 0 ]; then
    PS1='\\[\\e[1;31m\\]\\u@\\h\\[\\e[0m\\]:\\w\\$ '
else
    PS1='\\[\\e[1;32m\\]\\u@\\h\\[\\e[0m\\]:\\[\\e[1;34m\\]\\w\\[\\e[33m\\]$(__git_branch)\\[\\e[0m\\]\\$ '
fi

# Aliases
alias ll='ls -alF --color=auto'
alias la='ls -A'
alias grep='grep --color=auto'
alias art='php $APP_DIR/current/artisan'
alias logs='tail -F $APP_DIR/storage/logs/laravel.log'
alias dc='docker compose'

# Jump to the current release and show its version.
release() {
    cd "$APP_DIR/current" || return 1
    printf 'release: %s\\n' "$(basename "$(readlink -f .)")"
}

# Quick disk usage report for the storage folder.
dusage() {
    du -sh "\${1:-$APP_DIR/storage}"/* 2>/dev/null | sort -h | tail -n "\${2:-10}"
}

if [ -f /usr/share/bash-completion/bash_completion ]; then
    . /usr/share/bash-completion/bash_completion
fi

[ -f ~/.bash_aliases ] && . ~/.bash_aliases
`,
  '/home/deploy/.ssh/config': `# SSH client configuration for the deploy user.

Host *
    ServerAliveInterval 30
    ServerAliveCountMax 4
    AddKeysToAgent yes
    IdentitiesOnly yes
    HashKnownHosts yes
    ControlMaster auto
    ControlPath ~/.ssh/cm-%r@%h:%p
    ControlPersist 10m

Host bastion
    HostName 203.0.113.10
    User deploy
    Port 2222
    IdentityFile ~/.ssh/id_ed25519_bastion

Host web-1
    HostName 10.0.0.11
    User deploy
    ProxyJump bastion
    IdentityFile ~/.ssh/id_ed25519

Host web-2 web-3
    HostName %h.internal.example.test
    User deploy
    ProxyJump bastion
    IdentityFile ~/.ssh/id_ed25519

Host db-primary
    HostName 203.0.113.21
    User ops
    Port 22
    IdentityFile ~/.ssh/id_rsa_ops
    LocalForward 13306 127.0.0.1:3306

Host legacy
    HostName 203.0.113.99
    User root
    Port 2200
    IdentityFile ~/.ssh/legacy_key
    KexAlgorithms +diffie-hellman-group14-sha1
    HostKeyAlgorithms +ssh-rsa
    StrictHostKeyChecking accept-new
`,
  '/home/deploy/notes.txt': `TODO / notes -- deploy box
==========================

This week
---------
[x] Rotate the staging TLS certificate (expires 2026-11-20)
[x] Move backups to the new bucket (s3://backups.example.test/acme/)
[ ] Upgrade PHP 8.3.12 -> 8.3.14 on web-1, web-2, web-3
[ ] Raise client_max_body_size to 128m for the import feature
[ ] Find out why the 10:42 deploy on 07/11 lost the DB connection for ~6s
[ ] Clean up storage/exports on legacy (disk at 93%)

Later
-----
- Replace the legacy reports box (203.0.113.99) before the end of the year.
- Write a runbook for "migration lock stuck".
- Ask the platform team about moving Redis to a managed instance.
- Look into the slow invoices query (team_id + issued_on index?).
- The sidecar /metrics endpoint should be scraped every 15s, not 60s.

Useful commands
---------------
  ./deploy.sh status
  ./deploy.sh deploy --env staging
  art queue:failed | head
  sudo systemctl reload php8.3-fpm
  journalctl -u nginx --since "1 hour ago"

Contacts
--------
  On-call:   oncall@example.com
  Platform:  platform@example.com   (#acme-api)
  Finance:   finance@example.com    (monthly reports)

Remember: never edit .env on the hosts directly -- it lives in shared/ and is
managed by deploy.sh. Last updated 2026-11-07.
`,
  '/home/deploy/.profile': `# ~/.profile: executed by the command interpreter for login shells.
# This file is not read by bash(1) if ~/.bash_profile or ~/.bash_login exists.

# the default umask is set in /etc/profile; we want group-readable files
umask 022

# if running bash, include .bashrc if it exists
if [ -n "$BASH_VERSION" ]; then
    if [ -f "$HOME/.bashrc" ]; then
        . "$HOME/.bashrc"
    fi
fi

# set PATH so it includes user's private bin directories
if [ -d "$HOME/bin" ]; then
    PATH="$HOME/bin:$PATH"
fi

if [ -d "$HOME/.local/bin" ]; then
    PATH="$HOME/.local/bin:$PATH"
fi

# Composer global binaries
if [ -d "$HOME/.config/composer/vendor/bin" ]; then
    PATH="$HOME/.config/composer/vendor/bin:$PATH"
fi

export LANG=en_US.UTF-8
export LC_ALL=en_US.UTF-8
export TZ=UTC

# Start an ssh-agent for this login session if none is running.
if [ -z "$SSH_AUTH_SOCK" ]; then
    eval "$(ssh-agent -s)" > /dev/null
fi
`,
};

const LOG_BASE_MS = Date.UTC(2026, 10, 7, 10, 42, 1, 123);
/** Average spacing between consecutive lines, in ms (each line adds 0..LOG_STEP_MS-1 of jitter). */
const LOG_STEP_MS = 37;
const LOG_COMPONENTS = ['http', 'http', 'http', 'http', 'db', 'cache', 'queue', 'auth'] as const;
const LOG_METHODS = ['GET', 'GET', 'GET', 'POST', 'GET', 'PUT', 'GET', 'DELETE'] as const;
/** Routes ending in '/' get a numeric id appended. */
const LOG_ROUTES = [
  '/api/users/',
  '/api/users',
  '/api/teams/',
  '/api/invoices/',
  '/api/invoices',
  '/api/exports/',
  '/healthz',
  '/api/search',
] as const;
const LOG_OK_STATUSES = [200, 200, 200, 200, 201, 204, 304, 200] as const;
const LOG_WARN_STATUSES = [404, 401, 403, 422, 429, 409] as const;
const LOG_ERROR_STATUSES = [500, 502, 503, 504] as const;
const LOG_DB_MESSAGES = [
  'query select users rows=',
  'query select invoices rows=',
  'query update teams rows=',
  'pool stats open=',
] as const;
const LOG_CACHE_MESSAGES = [
  'hit key=users:',
  'miss key=users:',
  'set key=teams:',
  'evict key=exports:',
] as const;
const LOG_QUEUE_MESSAGES = [
  'job processed SendInvoiceMail id=',
  'job processed BuildExport id=',
  'job released RetryWebhook id=',
  'job processed PruneSessions id=',
] as const;
const LOG_AUTH_MESSAGES = [
  'token verified user=',
  'login ok user=',
  'token refreshed user=',
  'logout user=',
] as const;

/** Cache of the "YYYY-MM-DDTHH:MM:SS." prefix for the current second (recomputed ~every 27 lines). */
let logCachedSecond = -1;
let logCachedPrefix = '';

function logHash(n: number): number {
  let h = Math.imul(n ^ 0x9e3779b9, 0x85ebca6b);
  h ^= h >>> 13;
  h = Math.imul(h, 0xc2b2ae35);
  h ^= h >>> 16;
  return h >>> 0;
}

function logTimestamp(ms: number): string {
  const second = Math.floor(ms / 1000);
  if (second !== logCachedSecond) {
    logCachedSecond = second;
    logCachedPrefix = new Date(second * 1000).toISOString().slice(0, 20);
  }
  const frac = ms - second * 1000;
  return logCachedPrefix + (frac < 10 ? '00' : frac < 100 ? '0' : '') + frac + 'Z';
}

/** A deterministic application log line generator for the large fake log files. */
export function logLine(index: number): string {
  const h = logHash(index);
  const g = logHash(h ^ 0x27d4eb2f);
  const ts = logTimestamp(LOG_BASE_MS + index * LOG_STEP_MS + (h % LOG_STEP_MS));

  const roll = (h >>> 8) % 1000;
  const level = roll < 700 ? 'INFO ' : roll < 880 ? 'DEBUG' : roll < 985 ? 'WARN ' : 'ERROR';
  const component = LOG_COMPONENTS[(h >>> 18) & 7]!;
  const id = 1 + (g % 5000);
  const pick = (g >>> 13) & 3;

  if (component === 'http') {
    const method = LOG_METHODS[(g >>> 16) & 7]!;
    const route = LOG_ROUTES[(g >>> 20) & 7]!;
    let status: number;
    let ms: number;
    if (level === 'ERROR') {
      status = LOG_ERROR_STATUSES[pick]!;
      ms = 1000 + (g % 29000);
    } else if (level === 'WARN ') {
      status = LOG_WARN_STATUSES[(g >>> 24) % 6]!;
      ms = 5 + (g % 900);
    } else {
      status = LOG_OK_STATUSES[(g >>> 24) & 7]!;
      ms = 1 + (g % 120);
    }
    const path = route.charCodeAt(route.length - 1) === 47 ? route + id : route;
    const req = (g >>> 0).toString(16).padStart(8, '0');
    return `${ts} ${level} [http] ${method} ${path} ${status} ${ms}ms req=${req}`;
  }

  if (level === 'ERROR') {
    switch (component) {
      case 'db':
        return `${ts} ${level} [db] connection refused host=10.0.0.21:3306 attempt=${1 + pick}`;
      case 'cache':
        return `${ts} ${level} [cache] redis timeout after ${200 + (g % 800)}ms`;
      case 'queue':
        return `${ts} ${level} [queue] job failed BuildExport id=${id}: max attempts exceeded`;
      default:
        return `${ts} ${level} [auth] signature mismatch for token user=${id}`;
    }
  }
  if (level === 'WARN ') {
    switch (component) {
      case 'db':
        return `${ts} ${level} [db] slow query table=invoices ${300 + (g % 2700)}ms`;
      case 'cache':
        return `${ts} ${level} [cache] memory at ${80 + (g % 19)}% evictions=${g % 400}`;
      case 'queue':
        return `${ts} ${level} [queue] backlog default=${100 + (g % 900)} jobs`;
      default:
        return `${ts} ${level} [auth] failed login user=${id} ip=203.0.113.${1 + (g % 254)}`;
    }
  }
  const messages =
    component === 'db'
      ? LOG_DB_MESSAGES
      : component === 'cache'
        ? LOG_CACHE_MESSAGES
        : component === 'queue'
          ? LOG_QUEUE_MESSAGES
          : LOG_AUTH_MESSAGES;
  return `${ts} ${level} [${component}] ${messages[pick]!}${id} ${1 + (g % 40)}ms`;
}
