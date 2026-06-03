"""
generate_data.py — un solo uso para crear clima.csv.

Run UNA SOLA VEZ:
    python3 generate_data.py
"""
import pandas as pd
import numpy as np

np.random.seed(42)
rows = []
ciudades = {"El Chaltén": (5, 4), "Bariloche": (10, 5), "Buenos Aires": (18, 7)}
for mes in range(1, 13):
    for ciudad, (mean_temp, std_temp) in ciudades.items():
        for dia in range(1, 29):
            offset = 8 * np.sin((mes - 1) * np.pi / 6)
            temp = np.random.normal(mean_temp + offset, std_temp)
            rows.append(
                {
                    "fecha": f"2026-{mes:02d}-{dia:02d}",
                    "ciudad": ciudad,
                    "temperatura_c": round(temp, 1),
                    "humedad_pct": round(np.random.uniform(30, 90), 1),
                }
            )

df = pd.DataFrame(rows)
df.to_csv("clima.csv", index=False)
print(f"escribí {len(df)} filas a clima.csv")
