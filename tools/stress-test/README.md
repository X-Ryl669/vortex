# Vortex Stress Test Infrastructure

## Yaratilish maqsadi
Pairing va reconnect'ni real-world stress scenariylarida test qilish,
hidden bug'larni topish.

## Papka strukturasi
- `run-all.sh`: Barcha testlarni ketma-ket ishga tushiradi
- `scenarios/`: Alohida stress test skriptlari
- `monitors/`: (Rejada) Daemon va Android app uchun resource monitorlar
- `reports/`: Test loglari va natijalari (avtomatik yaratiladi)

## Qanday ishga tushiriladi

1. Linux daemon (L1) va Android app o'rtasida avvaldan pairing qilinganiga ishonch hosil qiling.
2. Android telefoni USB orqali ulangan (adb ishlashi kerak).
3. Va testlarni boshlang:

```bash
cd tools/stress-test
./run-all.sh
```

Natijalar `reports/summary.md` fayliga yoziladi.
