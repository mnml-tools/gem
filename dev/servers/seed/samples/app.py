"""Synthetic Python service used by the Gem test servers."""

from __future__ import annotations

import json
import logging
from dataclasses import dataclass, field

log = logging.getLogger(__name__)


@dataclass
class Job:
    name: str
    retries: int = 3
    tags: list[str] = field(default_factory=list)

    def to_json(self) -> str:
        return json.dumps({"name": self.name, "retries": self.retries, "tags": self.tags})


def main() -> None:
    jobs = [Job("reindex", tags=["nightly"]), Job("cleanup", retries=1)]
    for job in jobs:
        log.info("queued %s", job.to_json())


if __name__ == "__main__":
    main()
