# AD106 桌上型（RTX 4060 Ti 16GB，`10de:2805` / ASUS `1043:8921`）：全链实测成功

2026-10-28，单机单卡，Windows 11 + Intel NUC13RNGi9。用 v2.1.0 release 构建，
`-Family ad102` 资产管理线现场再生。完整报告、四份启动日志与测量脚本在
<https://github.com/holykylin/rtx4060ti-nuc13-unlock>（不含任何 NVIDIA 固件）。

## 結果

- 一次事务全链通过：`MAILBOX0=0x67b00001`、四条 PLM 读回符合预期、`SS0=0x88888888` /
  `SS1=0x8` 写入并经 FLR 存活，随后链回 `bootmgfw`，Windows 原版驱动 617.42 正常绑定。
- Windows 侧同一 boot：FP16→FP32 累加 GEMM 8192³ **45.78 → 75.24 TFLOPS（+64%）**，
  BF16 44.84 → 68.80，**FP32 对照 11.95 → 11.87（不变）**。
- 75 s 满载：中位 **75.88 TFLOPS**，功耗中位 149.9 W / 峰值 156 W（板限 165 W），
  GPU 最高 72 °C，SM 稳在约 2310 MHz。
- 复现：冷启两次（手动空格、武装模式 5 s 倒计时）结果一致（75.24 / 75.17）；
  休眠唤醒后固件重新 POST、U 盘再跑一轮，8192³ 72.57。

## 環境

| 項目 | 內容 |
| --- | --- |
| GPU | ASUS GeForce RTX 4060 Ti 16 GB，`10de:2805`，子系统 `1043:8921`，16,380 MiB，165 W |
| 位置 | GPU `0000:01:00.0`；上游桥 `8086:a70d` @ `0000:00:01.0`（pri/sec/sub = 0/1/1） |
| VBIOS / 驅動 | `95.06.25.00.d4` / 617.42 |
| 平台 | Intel NUC13RNGi9 / NUC13SBBi9，BIOS `SBRPL579.0068.2026.0625.1114`，iGPU `8086:a780` 同时在位 |
| 開機 | UEFI，Secure Boot 关闭；Windows 11 build 26300；PyTorch 2.12.1+cu130 |
| 建置 | v2.1.0 release `BOOTX64.EFI`（sha256 `b5e96f4e…c9dea`）+ `make-usbroot.ps1 -Family ad102` |

## 關鍵 log

```
DMA-PATH[pre-fire]: bridge 8086:a70d command 0x0003 status 0x0010 pri/sec/sub=0/1/1
BRIDGE-BME: Some((0, 0, 1, 0)) command 0x0003 -> 0x0007
BOOTER forensic: CPUCTL=0x00000010 MAILBOX0=0x67b00001 EXCI=0x00900100 TRANSCFG=0x00000000
POST-BOOT: FEAT_PLM=0xffffffff SS0=0x76111154 SS1=0x4
POST-BOOT: SEC2 MAILBOX0=0x67b00001 WPR2_HI=0x3ffae00
POST-BOOT: WPRCFG=0xfffff0ff FBPA=0xffffff0f WPR=0xffffffff
TX: SELECTOR RESULT: SS0=0x88888888 SS1=0x8
POST-FLR selectors: ss0=0x88888888 ss1=0x8
POST-FLR: verified (selectors survive AON)
FINAL: transaction complete, all DMA released
HANDOFF-OUT: ss0=0x88888888 ss1=0x8 pmcsr=0x0008 dstate=D0 | plm_open=true ss0_unlock=true ss1_unlock=true
```

冷态采样（只读）：chip `0x196000a1`、`FB_SIZE_MIB=0x3ffc`、`VGA_WORKSPACE=0x1`、
`FEAT 0x823804=0xffffff8f`、`SS0=0x76111154`、`SS1=0x4`；
`SEC2_CPUCTL=0xbadf5620 [BADF_VARIANT]`、TU10x 偏移 `0x409650/0x409664 = 0xbadf5040`
（Ada 上属预期）。

## 兩個附帶觀察

1. **上游橋 Bus Master 不是恆定值**。同一台機器，當天早些時候的只讀採集讀到
   `command=0x0007`（BME 已開），而解鎖那次啟動讀到 `0x0003`（BME 關）；靠
   `BridgeMaster` 自動置位成 `0x0007` 之後 booter 才拿到 `0x67b00001`。
   也就是說 PR #5 在這種「橋狀態隨啟動路徑變化」的固件上是**必需**而非可選。
2. **控制台 GOP 不在目標卡上時不會黑屏**。日誌 `DETACH: GOP handle … is not on the
   target GPU (multi-GPU) - left untouched`：本機 iGPU 同時在位，工具判定該 GOP 不屬
   目標卡並保持連線，因此整個過程全程有屏（與筆記本單卡路徑的黑屏窗口不同）。

## 局限

- 單機、單卡、單一 VBIOS，長時間穩定性與溫度/功耗包絡沒有做專項測試（只跑了 75 s 滿載）。
- 端到端只對「未量化、能整塊駐留顯存、採樣段打滿」的 fp16/bf16 工作流有效：本機同一套
  ComfyUI 下，稠密 bf16 圖像工作流 3.39 → 2.16 s/it（×1.57），而 20 GB 流式 +
  W4A4/W4A8/int8/fp8 量化的視頻工作流 81.8 s/it 與原廠持平（預期：瓶頸在帶寬）。
- 本機 VBIOS 的 PCI ROM / PROM 三類 SHA-256 未公開（設備指紋）；如需核對，
  可按 `docs/HARDWARE.md` 的偏移自行採集。
- 本卡在 Windows 側的判據用功能測量（GEMM 吞吐）而非寄存器讀回：`tools/ssdiag` 需要的
  `pmxdrv.sys` 未在本次測試中安裝。兩種方法可以互相校驗（本機 45.8/75.2 兩態差 1.6×）。
