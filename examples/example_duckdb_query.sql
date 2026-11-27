CREATE TABLE oxymart ("Store" BIGINT, "Date" DATE, "Weekly_Sales" DOUBLE, "Holiday_Flag" BIGINT, "Temperature" DOUBLE, "Fuel_Price" DOUBLE, "CPI" DOUBLE, "Unemployment" DOUBLE);

-- Basic statistics
SELECT COUNT(*) AS total_records,
    COUNT(DISTINCT Store) AS unique_stores,
    MIN(Date) AS earliest_date,
    MAX(Date) AS latest_date,
    AVG(Weekly_Sales) AS avg_sales,
    MIN(Weekly_Sales) AS min_sales,
    MAX(Weekly_Sales) AS max_sales
FROM oxymart;
-- Sales summary by store
SELECT Store,
    COUNT(*) AS weeks_count,
    SUM(Weekly_Sales) AS total_sales,
    AVG(Weekly_Sales) AS avg_sales,
    MIN(Weekly_Sales) AS min_sales,
    MAX(Weekly_Sales) AS max_sales
FROM oxymart
GROUP BY Store
ORDER BY total_sales DESC;
-- Time-based analysis
SELECT YEAR(Date) AS year,
    MONTH(Date) AS month,
    SUM(Weekly_Sales) AS total_sales,
    AVG(Weekly_Sales) AS avg_sales
FROM oxymart
GROUP BY YEAR(Date),
    MONTH(Date)
ORDER BY year,
    month;
-- Holiday impact
SELECT CASE
        WHEN Holiday_Flag = 1 THEN 'Holiday'
        ELSE 'Non-Holiday'
    END AS day_type,
    COUNT(*) AS record_count,
    AVG(Weekly_Sales) AS avg_sales
FROM oxymart
GROUP BY Holiday_Flag;
-- Correlation analysis
SELECT 'Temperature' AS factor,
    CORR(Weekly_Sales, Temperature) AS correlation_with_sales
FROM oxymart
UNION ALL
SELECT 'Fuel_Price' AS factor,
    CORR(Weekly_Sales, Fuel_Price) AS correlation_with_sales
FROM oxymart
UNION ALL
SELECT 'CPI' AS factor,
    CORR(Weekly_Sales, CPI) AS correlation_with_sales
FROM oxymart
UNION ALL
SELECT 'Unemployment' AS factor,
    CORR(Weekly_Sales, Unemployment) AS correlation_with_sales
FROM oxymart
ORDER BY ABS(correlation_with_sales) DESC;
-- Top and bottom performing store-weeks
(
    SELECT Store,
        Date,
        Weekly_Sales,
        Temperature,
        Fuel_Price,
        Holiday_Flag
    FROM oxymart
    ORDER BY Weekly_Sales DESC
    LIMIT 5
)
UNION ALL
(
    SELECT Store,
        Date,
        Weekly_Sales,
        Temperature,
        Fuel_Price,
        Holiday_Flag
    FROM oxymart
    ORDER BY Weekly_Sales ASC
    LIMIT 5
);