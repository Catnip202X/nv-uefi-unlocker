# 社区验证汇总与文档勘误

截至 2026-10-28。以下内容整理自仓库 issue 和已合入报告，属于报告者提供的实机记录，维护者未逐台复测。设备 ID 路由放行、EFI 读回成功、进入系统后的性能收益是不同层次的证据；同一 ID 换平台也可能得到不同结果。

## 硬件记录

吞吐单位为 TFLOPS。不同脚本、矩阵规模和功耗条件之间不作横向排名。

| 来源 | GPU / 版本 | 平台与进入方式 | 固件与系统侧证据 | 局限 |
| --- | --- | --- | --- | --- |
| [#3](https://github.com/physicsdolphin/nv-uefi-unlocker/issues/3#issuecomment-6434240012) | RTX 4070 SUPER，AD104 `2783`；v0.1.4 | Palit，MSI B760 TOMAHAWK WIFI，i5-13400F，Windows 11，驱动 621.01，220 W；每次冷启 | 桥 BME 修复后 replay/unlock 通过；marker、SS0/SS1、FLR 后出口读回正确。PyTorch/cuBLAS，7 次取中位数，8192：FP16→FP32 累加 70.5→100.1、BF16 71.0→107.0、TF32 35.5→48.4；FP16 累加与 FP32 SIMT 对照基本不变 | 报告者提供关键行，表示完整日志可提供；1024³ 对 FP64 参考的检查前后逐位一致，不代表所有工作负载均已验证 |
| [#27](https://github.com/physicsdolphin/nv-uefi-unlocker/issues/27) | RTX 3080 Ti Laptop 16GB，GA103M `2420`；v2.1.0 | MSI Vector GP76 12UHSO，独显直连，175 W，AC；Windows 11，驱动 621.01，VBIOS `94.03.19.00.90`；独立 FAT32 分区经 EFI Shell 进入，未写系统 ESP | replay 通过，PLM 翻转、选择器读回及 FLR 存活；8192 GEMM 各 3 轮中位数：FP16 49.23→77.80/80.35，BF16 50.35→77.64/80.84；FP32 12.80→12.64/12.68，数值检查通过。[日志与方法](https://gist.github.com/INsincere123/f0778d5378fdb5bb64da1aaabdefa341) | 未做 Windows 内 FEAT 读回或长期稳定性验证。报告者曾同会话再次执行后出现 BIOS 显示异常、NVMe 不可见，EC 重置恢复；首次日志被覆盖，原因不明。后续单次 replay 与两次解锁正常 |
| [#28](https://github.com/physicsdolphin/nv-uefi-unlocker/issues/28) | RTX 3070 Laptop 8GB，GA104M `249d`；v2.1.0 | AMD Cezanne，AMI，MUX 独显直连，140 W；Windows，驱动 616.56 | PLM 翻转约 158 ms，SS0/SS1、签名还原、FLR 和 HANDOFF-OUT 读回正确；cuBLAS FP16 + FP32 累加：8192 为 35.43→56.04，4096 为 37.82→60.96 | 混合模式下仅枚举音频功能，需直连。与 #6 的另一台同 ID 混合输出机器结果不同；不能据此保证全部 249d 平台兼容 |
| [#29](https://github.com/physicsdolphin/nv-uefi-unlocker/issues/29) | RTX 3080，GA102 `2206`；v2.1.0 | MSI MEG Z490 UNIFY，单卡直连，340 W；Windows 10，驱动 610.74，VBIOS `94.02.26.c0.b6` | WPR2、PLM 翻转、选择器、签名还原及 FLR/出口读回通过，Windows 设备状态正常。torch/cuBLAS FP16，median / best：4096 为 48.36/56.83→104.55/105.93；8192 为 63.96/65.65→99.67/106.44；12288 为 66.10/66.69→101.98/104.57 | 62 秒负载持续 99.01，约 338 W、80 °C，报告 SW power cap；这次短测不等于长期稳定性验证，功耗墙对收益的解释保留为报告者判读 |
| [#31 已合入报告](evidence/ad106-2805-nuc13/report.md) | RTX 4060 Ti 16GB，AD106 `2805`，ASUS `1043:8921`；v2.1.0 | Intel NUC13RNGi9，Windows 11，驱动 617.42 | marker、PLM、选择器及 FLR 存活通过；同次启动 8192 GEMM FP16 45.78→75.24、BF16 44.84→68.80，FP32 11.95→11.87；75 秒负载，最高 72 °C | 单卡单 VBIOS；Windows 侧使用性能测量，未安装 ssdiag 的内核通道；ComfyUI 稠密 bf16 3.39→2.16 s/it，报告的流式量化视频工作流无收益 |

已有的 [#8（4070 `2709`）](https://github.com/physicsdolphin/nv-uefi-unlocker/issues/8)、[#11（3060 Ti LHR `2489`）](https://github.com/physicsdolphin/nv-uefi-unlocker/issues/11)、[#12（2060 Super `1f06`）](https://github.com/physicsdolphin/nv-uefi-unlocker/issues/12)记录保留在 README；[#2](https://github.com/physicsdolphin/nv-uefi-unlocker/issues/2) 的 Turing 计算解锁与 BAR1 扩展是不同工作，后者没有随计算解锁一并实现。

## 仍待复测的反馈

- [#6](https://github.com/physicsdolphin/nv-uefi-unlocker/issues/6)：GA10x 原先等待 HALT/marker 的误判已改为 PLM 翻转判据。但另一台 Hasee/Clevo、无 MUX 的混合输出 `249d` 在 v2.1.0 EFI 成功后进入 Windows 无收益。可用 [Windows ssdiag](../tools/ssdiag/README.md) 或 Linux `tools/diag-unlock.py` 与 HANDOFF-OUT 对照，区分选择器被重写、PLM 重锁或设备失电；尚无该机器修复成功的回报。`--reapply` 是写操作，仅在 PLM 仍开放时尝试并读回，不应把工具已提供写成故障已修复。
- [#10](https://github.com/physicsdolphin/nv-uefi-unlocker/issues/10)：`2820` 已纳入 AD106 路由，未收到该魔改卡的成功复测；保持未验证。
- [#14](https://github.com/physicsdolphin/nv-uefi-unlocker/issues/14)、[#17](https://github.com/physicsdolphin/nv-uefi-unlocker/issues/17)、[#19](https://github.com/physicsdolphin/nv-uefi-unlocker/issues/19)：早期 HIGH 标签不等于实际地址超过 4 GiB。v2.1.2 改用 GetMemoryMap + AllocateAddress；#17 已确认实际高地址分配成功，仍返回 0x91，22GB 卡却使用 8GB 布局。v2.1.4 提供容量覆盖标记，但这不构成该卡已解锁的证据。地址、布局与最终读回应分别核对，不能把所有 0x91 都归于低地址。

## 使用说明与实现边界

- USB 启动文件位于 `\EFI\BOOT\BOOTX64.EFI`，资产与标记文件位于盘根。maker 当前平铺输出，还保留中间文件；按所选家族的 `expected` 清单复制，另建 EFI 启动目录。#28/#29 均指出了这处歧义。
- 普通确认只接受空格；回车、其他键或 30 秒超时均中止事务。存在 `i_know_what_i_am_doing.txt` 时改为 5 秒自动运行，任意键中止。回放模式不含解锁载荷，但仍会启动固件和复位，不等于只读采样。
- #29 报告 Windows 10 / PowerShell 5.1 的 TLS 握手失败，可尝试 WSL/Git Bash 运行 bash maker。继续使用脚本钉住的输入与 hash，不把传输问题当作换固件版本的理由。scrubber 的 handoff=1 警告在部分成功报告中也出现；是否完成事务仍须看后续 FWSEC、家族执行判据、选择器和交棒结果。
- [#4](https://github.com/physicsdolphin/nv-uefi-unlocker/issues/4)：超时确认语义已修复；GA10x/Ada 的 WPR2 守卫取决于本轮是否执行 pre-FLR，Turing 当前仍允许 WPR2 已建立的入口。它不是所有家族统一拒绝非零 WPR2 的实现。守卫前的 detach/claim 也有配置写入，不能将所有拒绝路径描述为全程零写。
- PROM CRC32 日志是采样指纹，当前 loader 未实现安全宪法要求的逐卡 ROM/PROM SHA-256 拒绝门。家族路由也不能替代这种身份验证。该要求仍然有效，此处只说明实现差距。
- [#20](https://github.com/physicsdolphin/nv-uefi-unlocker/issues/20)：现行 loader 使用 portal 装载；保留的 Falcon DMA 实验代码不是已验证执行路径。[#25](https://github.com/physicsdolphin/nv-uefi-unlocker/issues/25)：host 描述符解析实现 V2/V3，V1 尚未实现；encrypted 位解析后未保留或强制检查。
- [#22](https://github.com/physicsdolphin/nv-uefi-unlocker/issues/22)：模拟器的部分 verdict 来自故障开关、marker 由模拟侧写入，复位模型也存在待审问题。宿主测试通过不等于所有事务判据已独立验证，更不能代替真机读回。
- [#18](https://github.com/physicsdolphin/nv-uefi-unlocker/issues/18) 的诊断字节偏移/掩码、[#21](https://github.com/physicsdolphin/nv-uefi-unlocker/issues/21) 的 ROM 边界检查、[#23](https://github.com/physicsdolphin/nv-uefi-unlocker/issues/23) 的 bash 多卡 GSP 门、[#24](https://github.com/physicsdolphin/nv-uefi-unlocker/issues/24) 的 LIC 描述符处理仍有开放问题。本次只记录文档边界，不宣称这些代码问题已修复。

## 历史记录如何阅读

早期报告中的 IOMMU/TSME 对 0x89 的归因已被后续桥 BME 实验推翻，详见[根因报告](bridge-bme-root-cause.md)。Turing 的真 FEAT 块位于 `0x4096xx`；早期对 `0x823xxx` 的 BADF 观察不能当作真 FEAT 块全锁的证明。历史日期、原始日志与测量数据保留，以勘误说明更新后的认识。

这些文档更新不关闭仍需代码审查或真机复测的 issue。
