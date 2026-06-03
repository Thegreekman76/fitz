"""
weather.py — helpers para Fitz que devuelven list[dict] / dict
desde análisis con pandas + numpy.

Convertimos los DataFrame y los objetos numpy en list[dict] explícito
porque Fitz solo auto-marshalea primitivos + List/Map (Fase 8.2).
DataFrame es opaco; tenés que convertir adentro de Python antes de
devolver.
"""
import pandas as pd
import numpy as np
from pathlib import Path

_CSV_PATH = Path(__file__).parent / "clima.csv"


def load_weather():
    """Sanity check: cuántas filas tiene el CSV."""
    df = pd.read_csv(_CSV_PATH)
    return len(df)


def stats_por_mes_y_ciudad():
    """
    Lee clima.csv, agrupa por mes+ciudad, calcula promedios.
    Devuelve list[dict] para que Fitz marshale a
    List<Map<Str, Any>> o coercione a List<StatRow>.
    """
    df = pd.read_csv(_CSV_PATH)
    df["fecha"] = pd.to_datetime(df["fecha"])
    df["mes"] = df["fecha"].dt.month

    grouped = (
        df.groupby(["mes", "ciudad"])
        .agg(
            temp_promedio=("temperatura_c", "mean"),
            temp_desvio=("temperatura_c", "std"),
            humedad_promedio=("humedad_pct", "mean"),
            muestras=("temperatura_c", "count"),
        )
        .reset_index()
    )

    for col in ["temp_promedio", "temp_desvio", "humedad_promedio"]:
        grouped[col] = grouped[col].round(2)

    return grouped.to_dict(orient="records")


def percentiles_de_ciudad(ciudad: str):
    """
    Devuelve p25/p50/p75 de temperatura para una ciudad específica.
    """
    df = pd.read_csv(_CSV_PATH)
    serie = df[df["ciudad"] == ciudad]["temperatura_c"]
    if len(serie) == 0:
        raise ValueError(f"ciudad desconocida: {ciudad}")
    return {
        "ciudad": ciudad,
        "p25": round(float(np.percentile(serie, 25)), 2),
        "p50": round(float(np.percentile(serie, 50)), 2),
        "p75": round(float(np.percentile(serie, 75)), 2),
    }
