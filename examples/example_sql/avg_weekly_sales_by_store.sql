/*
  oxy:
    database: local
    embed:
      - What is the total weekly sales for all stores
*/
SELECT SUM(Weekly_Sales) AS total_weekly_sales
FROM oxymart;