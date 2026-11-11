<?php

declare(strict_types=1);

namespace App;

use App\Http\Kernel;
use App\Http\Request;

require __DIR__ . '/vendor/autoload.php';

/**
 * Front controller for the synthetic demo application.
 */
final class Application
{
    private const VERSION = '1.4.2';

    public function __construct(private readonly Kernel $kernel)
    {
    }

    public function run(Request $request): void
    {
        $response = $this->kernel->handle($request);
        foreach ($response->headers() as $name => $value) {
            header(sprintf('%s: %s', $name, $value));
        }
        echo $response->body();
    }
}

$app = new Application(new Kernel(['debug' => getenv('APP_DEBUG') === '1']));
$app->run(Request::fromGlobals());
