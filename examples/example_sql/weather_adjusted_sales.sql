/*
  oxy:
    database: local
    embed:
      - How does rain affect sales
      - Sales on rainy versus dry days
      - Weather adjusted sales by rain
      - Effect of rain on dine-in and delivery mix
*/
-- Weather-adjusted sales: real Open-Meteo weather joined to daily sales by
-- location + date. Shows how rain shifts revenue and the dine-in/delivery mix.
SELECT
    CASE WHEN w.precip > 1.0 THEN 'Rain' ELSE 'Dry' END AS weather,
    COUNT(*)                                                AS location_days,
    ROUND(AVG(s.net_sales), 0)                              AS avg_net_sales,
    ROUND(AVG(s.dine_in_sales), 0)                          AS avg_dine_in,
    ROUND(AVG(s.delivery_sales), 0)                         AS avg_delivery,
    ROUND(100.0 * AVG(s.delivery_sales) / AVG(s.net_sales), 1) AS delivery_mix_pct
FROM sales_daily s
JOIN weather_daily w ON s.loc_date = w.loc_date
GROUP BY weather
ORDER BY weather;
