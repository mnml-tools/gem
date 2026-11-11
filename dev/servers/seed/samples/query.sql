-- Synthetic query used by the Gem test servers.
WITH recent AS (
    SELECT o.id, o.customer_id, o.total, o.created_at
    FROM orders AS o
    WHERE o.created_at >= now() - INTERVAL '30 days'
)
SELECT c.name, count(r.id) AS orders, sum(r.total) AS revenue
FROM customers AS c
JOIN recent AS r ON r.customer_id = c.id
GROUP BY c.name
HAVING sum(r.total) > 100
ORDER BY revenue DESC
LIMIT 20;
