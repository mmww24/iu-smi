using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices;
using System.Text.RegularExpressions;
using Microsoft.Win32;

internal sealed class Program
{
    private static readonly int[] AllowedIntervals = new int[] { 1, 5, 10, 15, 30, 60 };
    private static readonly Regex PidRegex = new Regex(@"pid_(\d+)", RegexOptions.IgnoreCase | RegexOptions.Compiled);
    private static readonly Regex EngineRegex = new Regex(@"engtype_([^_)]*)", RegexOptions.IgnoreCase | RegexOptions.Compiled);
    private static readonly Dictionary<int, string> ProcessNameCache = new Dictionary<int, string>();

    private static bool _stopRequested;

    private static int Main(string[] args)
    {
        Options options;
        try
        {
            options = Options.Parse(args);
        }
        catch (ArgumentException ex)
        {
            Console.Error.WriteLine("iu-smi: " + ex.Message);
            Console.Error.WriteLine("Try: iu-smi --help");
            return 2;
        }

        if (options.Help)
        {
            PrintHelp();
            return 0;
        }

        Console.CancelKeyPress += delegate(object sender, ConsoleCancelEventArgs eventArgs)
        {
            eventArgs.Cancel = true;
            _stopRequested = true;
        };

        do
        {
            Sample sample = Sampler.Collect(options.IntervalSeconds);

            if (!options.NoClear && !options.Once)
            {
                TryClear();
            }

            Renderer.Print(sample, options);

            if (options.Once)
            {
                break;
            }

            Console.WriteLine();
            Console.WriteLine("Press q to quit. Ctrl+C also exits.");
            if (ReadQuitKey())
            {
                break;
            }
        }
        while (!_stopRequested);

        return 0;
    }

    private static bool ReadQuitKey()
    {
        try
        {
            if (Console.IsInputRedirected || !Console.KeyAvailable)
            {
                return false;
            }

            ConsoleKeyInfo key = Console.ReadKey(true);
            return key.Key == ConsoleKey.Q;
        }
        catch (InvalidOperationException)
        {
            return false;
        }
    }

    private static void TryClear()
    {
        try
        {
            Console.Clear();
        }
        catch (IOException)
        {
        }
        catch (InvalidOperationException)
        {
        }
    }

    private static void PrintHelp()
    {
        Console.WriteLine("iu-smi - Intel Ultra CPU/GPU/NPU live usage monitor");
        Console.WriteLine();
        Console.WriteLine("Usage:");
        Console.WriteLine("  iu-smi                 Live view, 1 second refresh");
        Console.WriteLine("  iu-smi --follow 5      Live view, 5 second refresh");
        Console.WriteLine("  iu-smi --once          One snapshot sampled over 1 second");
        Console.WriteLine();
        Console.WriteLine("Options:");
        Console.WriteLine("  -f, --follow <sec>     Refresh interval: 1, 5, 10, 15, 30, or 60 seconds");
        Console.WriteLine("  -i, --interval <sec>   Same as --follow");
        Console.WriteLine("  --once                 Print one snapshot and exit");
        Console.WriteLine("  --top <n>              Number of busy engine rows to show, default 10");
        Console.WriteLine("  --no-clear             Do not clear the console between live frames");
        Console.WriteLine("  -h, --help             Show this help");
    }

    private sealed class Options
    {
        public int IntervalSeconds = 1;
        public bool Once;
        public bool NoClear;
        public bool Help;
        public int Top = 10;

        public static Options Parse(string[] args)
        {
            Options options = new Options();

            for (int i = 0; i < args.Length; i++)
            {
                string arg = args[i];
                if (arg == "-h" || arg == "--help" || arg == "/?")
                {
                    options.Help = true;
                }
                else if (arg == "--once")
                {
                    options.Once = true;
                }
                else if (arg == "--no-clear")
                {
                    options.NoClear = true;
                }
                else if (arg == "-f" || arg == "--follow" || arg == "-i" || arg == "--interval")
                {
                    if (i + 1 >= args.Length)
                    {
                        throw new ArgumentException(arg + " requires a seconds value");
                    }

                    options.IntervalSeconds = ParseInterval(args[++i]);
                }
                else if (arg == "--top")
                {
                    if (i + 1 >= args.Length)
                    {
                        throw new ArgumentException("--top requires a row count");
                    }

                    int top;
                    if (!int.TryParse(args[++i], NumberStyles.None, CultureInfo.InvariantCulture, out top) || top < 0)
                    {
                        throw new ArgumentException("--top must be a non-negative integer");
                    }

                    options.Top = top;
                }
                else
                {
                    throw new ArgumentException("unknown argument: " + arg);
                }
            }

            return options;
        }

        private static int ParseInterval(string text)
        {
            int seconds;
            if (!int.TryParse(text, NumberStyles.None, CultureInfo.InvariantCulture, out seconds))
            {
                throw new ArgumentException("interval must be a number of seconds");
            }

            if (!AllowedIntervals.Contains(seconds))
            {
                throw new ArgumentException("interval must be one of: " + string.Join(", ", AllowedIntervals) + " seconds");
            }

            return seconds;
        }
    }

    private static class Renderer
    {
        public static void Print(Sample sample, Options options)
        {
            string cpuName = GetCpuName();
            Console.WriteLine("iu-smi  {0}  interval={1}s", sample.Timestamp.ToString("yyyy-MM-dd HH:mm:ss"), sample.IntervalSeconds);
            Console.WriteLine(cpuName);
            Console.WriteLine();

            Console.WriteLine("CPU        {0,6:0.0}%     RAM {1,6:0.00}/{2,6:0.00} GiB  {3,6:0.0}%",
                sample.CpuPercent,
                ToGiB(sample.MemoryUsedBytes),
                ToGiB(sample.MemoryTotalBytes),
                sample.MemoryPercent);

            Console.WriteLine("GPU busy   {0,6:0.0}%     3D {1,6:0.0}%  Compute {2,6:0.0}%  Copy {3,6:0.0}%  Video {4,6:0.0}%  Other {5,6:0.0}%",
                Cap(sample.GpuBusyPercent),
                Cap(Get(sample.GpuByEngine, "3d")),
                Cap(Get(sample.GpuByEngine, "compute")),
                Cap(Get(sample.GpuByEngine, "copy")),
                Cap(GetVideo(sample.GpuByEngine)),
                Cap(GetOther(sample.GpuByEngine)));

            if (sample.SeenNpu)
            {
                Console.WriteLine("NPU neural {0,6:0.0}%", Cap(sample.NpuNeuralPercent));
            }
            else
            {
                Console.WriteLine("NPU neural    n/a      no engtype_neural counter was visible in this sample");
            }

            Console.WriteLine("AdapterMem shared {0,6:0.00} GiB  dedicated {1,6:0.00} GiB",
                ToGiB(sample.AdapterSharedBytes),
                ToGiB(sample.AdapterDedicatedBytes));

            if (sample.PdhWarnings.Count > 0)
            {
                Console.WriteLine();
                foreach (string warning in sample.PdhWarnings)
                {
                    Console.WriteLine("warning: " + warning);
                }
            }

            if (options.Top > 0)
            {
                Console.WriteLine();
                Console.WriteLine("Top engines");
                Console.WriteLine("  {0,-6} {1,-24} {2,-4} {3,-18} {4,8}", "PID", "Process", "Unit", "Engine", "Util%");
                foreach (ProcessEngineUsage usage in sample.ProcessEngines
                    .OrderByDescending(x => x.Percent)
                    .Take(options.Top))
                {
                    if (usage.Percent < 0.05)
                    {
                        continue;
                    }

                    Console.WriteLine("  {0,-6} {1,-24} {2,-4} {3,-18} {4,8:0.0}",
                        usage.Pid,
                        Trim(usage.ProcessName, 24),
                        usage.Unit,
                        Trim(usage.EngineType, 18),
                        Cap(usage.Percent));
                }
            }
        }

        private static string GetCpuName()
        {
            object value = Registry.GetValue(@"HKEY_LOCAL_MACHINE\HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString", null);
            if (value == null)
            {
                return "Intel CPU/GPU/NPU monitor";
            }

            return value.ToString().Trim();
        }

        private static double Get(Dictionary<string, double> values, string key)
        {
            double value;
            return values.TryGetValue(key, out value) ? value : 0.0;
        }

        private static double GetVideo(Dictionary<string, double> values)
        {
            double total = 0.0;
            foreach (KeyValuePair<string, double> item in values)
            {
                if (item.Key.StartsWith("video", StringComparison.OrdinalIgnoreCase))
                {
                    total += item.Value;
                }
            }

            return total;
        }

        private static double GetOther(Dictionary<string, double> values)
        {
            double total = 0.0;
            foreach (KeyValuePair<string, double> item in values)
            {
                string key = item.Key;
                if (key != "3d" && key != "compute" && key != "copy" && !key.StartsWith("video", StringComparison.OrdinalIgnoreCase))
                {
                    total += item.Value;
                }
            }

            return total;
        }

        private static string Trim(string value, int width)
        {
            if (string.IsNullOrEmpty(value))
            {
                return "";
            }

            return value.Length <= width ? value : value.Substring(0, width - 1) + "~";
        }

        private static double ToGiB(ulong bytes)
        {
            return bytes / 1073741824.0;
        }

        private static double Cap(double value)
        {
            if (double.IsNaN(value) || double.IsInfinity(value))
            {
                return 0.0;
            }

            if (value < 0.0)
            {
                return 0.0;
            }

            return value > 100.0 ? 100.0 : value;
        }
    }

    private static class Sampler
    {
        public static Sample Collect(int intervalSeconds)
        {
            Sample sample = new Sample();
            sample.Timestamp = DateTime.Now;
            sample.IntervalSeconds = intervalSeconds;

            Native.CpuTimes cpuStart = Native.ReadCpuTimes();
            IntPtr query = IntPtr.Zero;
            IntPtr gpuUtil = IntPtr.Zero;
            IntPtr dedicatedMem = IntPtr.Zero;
            IntPtr sharedMem = IntPtr.Zero;

            try
            {
                uint status = Native.PdhOpenQuery(null, UIntPtr.Zero, out query);
                if (status != Native.ERROR_SUCCESS)
                {
                    sample.PdhWarnings.Add("PdhOpenQuery failed: 0x" + status.ToString("x8"));
                }
                else
                {
                    AddCounter(query, @"\GPU Engine(*)\Utilization Percentage", out gpuUtil, sample.PdhWarnings);
                    AddCounter(query, @"\GPU Adapter Memory(*)\Dedicated Usage", out dedicatedMem, sample.PdhWarnings);
                    AddCounter(query, @"\GPU Adapter Memory(*)\Shared Usage", out sharedMem, sample.PdhWarnings);

                    Native.PdhCollectQueryData(query);
                }

                SleepSeconds(intervalSeconds);

                Native.CpuTimes cpuEnd = Native.ReadCpuTimes();
                ApplyCpu(sample, cpuStart, cpuEnd);
                ApplyMemory(sample);

                if (query != IntPtr.Zero)
                {
                    Native.PdhCollectQueryData(query);
                    ApplyGpu(sample, gpuUtil);
                    sample.AdapterDedicatedBytes = SumBytes(dedicatedMem);
                    sample.AdapterSharedBytes = SumBytes(sharedMem);
                }
            }
            finally
            {
                if (query != IntPtr.Zero)
                {
                    Native.PdhCloseQuery(query);
                }
            }

            return sample;
        }

        private static void AddCounter(IntPtr query, string path, out IntPtr counter, List<string> warnings)
        {
            counter = IntPtr.Zero;
            uint status = Native.PdhAddEnglishCounter(query, path, UIntPtr.Zero, out counter);
            if (status != Native.ERROR_SUCCESS)
            {
                warnings.Add(path + " unavailable: 0x" + status.ToString("x8"));
            }
        }

        private static void SleepSeconds(int intervalSeconds)
        {
            int remaining = intervalSeconds * 1000;
            while (remaining > 0 && !_stopRequested)
            {
                int slice = Math.Min(100, remaining);
                System.Threading.Thread.Sleep(slice);
                remaining -= slice;
            }
        }

        private static void ApplyCpu(Sample sample, Native.CpuTimes start, Native.CpuTimes end)
        {
            ulong idle = end.Idle - start.Idle;
            ulong kernel = end.Kernel - start.Kernel;
            ulong user = end.User - start.User;
            ulong total = kernel + user;
            sample.CpuPercent = total == 0 ? 0.0 : (total - idle) * 100.0 / total;
        }

        private static void ApplyMemory(Sample sample)
        {
            Native.MEMORYSTATUSEX mem = new Native.MEMORYSTATUSEX();
            mem.dwLength = (uint)Marshal.SizeOf(typeof(Native.MEMORYSTATUSEX));
            if (Native.GlobalMemoryStatusEx(ref mem))
            {
                sample.MemoryTotalBytes = mem.ullTotalPhys;
                sample.MemoryUsedBytes = mem.ullTotalPhys - mem.ullAvailPhys;
                sample.MemoryPercent = mem.dwMemoryLoad;
            }
        }

        private static void ApplyGpu(Sample sample, IntPtr gpuUtil)
        {
            if (gpuUtil == IntPtr.Zero)
            {
                return;
            }

            foreach (PdhValue value in Native.ReadFormattedDoubleArray(gpuUtil))
            {
                EngineIdentity identity = ParseEngine(value.Name);
                if (identity.Pid <= 0)
                {
                    continue;
                }

                string engineType = string.IsNullOrWhiteSpace(identity.EngineType) ? "other" : identity.EngineType.ToLowerInvariant();
                string unit = engineType == "neural" ? "NPU" : "GPU";
                double percent = value.Value < 0.0 ? 0.0 : value.Value;

                if (unit == "NPU")
                {
                    sample.SeenNpu = true;
                    sample.NpuNeuralPercent += percent;
                }
                else
                {
                    Add(sample.GpuByEngine, engineType, percent);
                }

                string key = identity.Pid.ToString(CultureInfo.InvariantCulture) + "|" + unit + "|" + engineType;
                ProcessEngineUsage usage;
                if (!sample.ProcessEngineIndex.TryGetValue(key, out usage))
                {
                    usage = new ProcessEngineUsage();
                    usage.Pid = identity.Pid;
                    usage.ProcessName = ResolveProcessName(identity.Pid);
                    usage.Unit = unit;
                    usage.EngineType = engineType;
                    sample.ProcessEngineIndex[key] = usage;
                    sample.ProcessEngines.Add(usage);
                }

                usage.Percent += percent;
            }

            double busy = 0.0;
            foreach (double percent in sample.GpuByEngine.Values)
            {
                busy = Math.Max(busy, percent);
            }

            sample.GpuBusyPercent = busy;
        }

        private static ulong SumBytes(IntPtr counter)
        {
            if (counter == IntPtr.Zero)
            {
                return 0;
            }

            double total = 0.0;
            foreach (PdhValue value in Native.ReadFormattedDoubleArray(counter))
            {
                if (value.Value > 0.0)
                {
                    total += value.Value;
                }
            }

            return total < 0.0 ? 0UL : (ulong)total;
        }

        private static EngineIdentity ParseEngine(string instance)
        {
            EngineIdentity identity = new EngineIdentity();

            Match pid = PidRegex.Match(instance ?? "");
            if (pid.Success)
            {
                int parsed;
                if (int.TryParse(pid.Groups[1].Value, NumberStyles.None, CultureInfo.InvariantCulture, out parsed))
                {
                    identity.Pid = parsed;
                }
            }

            Match engine = EngineRegex.Match(instance ?? "");
            if (engine.Success)
            {
                identity.EngineType = engine.Groups[1].Value;
            }

            return identity;
        }

        private static void Add(Dictionary<string, double> values, string key, double amount)
        {
            double current;
            values.TryGetValue(key, out current);
            values[key] = current + amount;
        }

        private static string ResolveProcessName(int pid)
        {
            string cached;
            if (ProcessNameCache.TryGetValue(pid, out cached))
            {
                return cached;
            }

            string name;
            try
            {
                using (Process process = Process.GetProcessById(pid))
                {
                    name = process.ProcessName;
                }
            }
            catch
            {
                name = "pid_" + pid.ToString(CultureInfo.InvariantCulture);
            }

            ProcessNameCache[pid] = name;
            return name;
        }
    }

    private sealed class Sample
    {
        public DateTime Timestamp;
        public int IntervalSeconds;
        public double CpuPercent;
        public ulong MemoryTotalBytes;
        public ulong MemoryUsedBytes;
        public double MemoryPercent;
        public double GpuBusyPercent;
        public double NpuNeuralPercent;
        public bool SeenNpu;
        public ulong AdapterSharedBytes;
        public ulong AdapterDedicatedBytes;
        public readonly Dictionary<string, double> GpuByEngine = new Dictionary<string, double>(StringComparer.OrdinalIgnoreCase);
        public readonly List<ProcessEngineUsage> ProcessEngines = new List<ProcessEngineUsage>();
        public readonly Dictionary<string, ProcessEngineUsage> ProcessEngineIndex = new Dictionary<string, ProcessEngineUsage>(StringComparer.OrdinalIgnoreCase);
        public readonly List<string> PdhWarnings = new List<string>();
    }

    private sealed class ProcessEngineUsage
    {
        public int Pid;
        public string ProcessName;
        public string Unit;
        public string EngineType;
        public double Percent;
    }

    private struct EngineIdentity
    {
        public int Pid;
        public string EngineType;
    }

    private struct PdhValue
    {
        public string Name;
        public double Value;
    }

    private static class Native
    {
        public const uint ERROR_SUCCESS = 0;
        private const uint PDH_MORE_DATA = 0x800007D2;
        private const uint PDH_FMT_DOUBLE = 0x00000200;

        [DllImport("pdh.dll", CharSet = CharSet.Unicode)]
        public static extern uint PdhOpenQuery(string dataSource, UIntPtr userData, out IntPtr query);

        [DllImport("pdh.dll", CharSet = CharSet.Unicode)]
        public static extern uint PdhAddEnglishCounter(IntPtr query, string fullCounterPath, UIntPtr userData, out IntPtr counter);

        [DllImport("pdh.dll")]
        public static extern uint PdhCollectQueryData(IntPtr query);

        [DllImport("pdh.dll")]
        public static extern uint PdhCloseQuery(IntPtr query);

        [DllImport("pdh.dll", CharSet = CharSet.Unicode)]
        private static extern uint PdhGetFormattedCounterArray(IntPtr counter, uint format, ref uint bufferSize, ref uint itemCount, IntPtr itemBuffer);

        [DllImport("kernel32.dll", SetLastError = true)]
        public static extern bool GlobalMemoryStatusEx(ref MEMORYSTATUSEX buffer);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool GetSystemTimes(out FILETIME idleTime, out FILETIME kernelTime, out FILETIME userTime);

        public static CpuTimes ReadCpuTimes()
        {
            FILETIME idle;
            FILETIME kernel;
            FILETIME user;
            if (!GetSystemTimes(out idle, out kernel, out user))
            {
                return new CpuTimes();
            }

            CpuTimes times = new CpuTimes();
            times.Idle = ToUInt64(idle);
            times.Kernel = ToUInt64(kernel);
            times.User = ToUInt64(user);
            return times;
        }

        public static IEnumerable<PdhValue> ReadFormattedDoubleArray(IntPtr counter)
        {
            List<PdhValue> values = new List<PdhValue>();
            if (counter == IntPtr.Zero)
            {
                return values;
            }

            uint bufferSize = 0;
            uint itemCount = 0;
            uint status = PdhGetFormattedCounterArray(counter, PDH_FMT_DOUBLE, ref bufferSize, ref itemCount, IntPtr.Zero);
            if (status != PDH_MORE_DATA || bufferSize == 0 || itemCount == 0)
            {
                return values;
            }

            IntPtr buffer = Marshal.AllocHGlobal((int)bufferSize);
            try
            {
                status = PdhGetFormattedCounterArray(counter, PDH_FMT_DOUBLE, ref bufferSize, ref itemCount, buffer);
                if (status != ERROR_SUCCESS)
                {
                    return values;
                }

                int itemSize = Marshal.SizeOf(typeof(PDH_FMT_COUNTERVALUE_ITEM_DOUBLE));
                for (int i = 0; i < itemCount; i++)
                {
                    IntPtr itemPtr = new IntPtr(buffer.ToInt64() + i * itemSize);
                    PDH_FMT_COUNTERVALUE_ITEM_DOUBLE item = (PDH_FMT_COUNTERVALUE_ITEM_DOUBLE)Marshal.PtrToStructure(itemPtr, typeof(PDH_FMT_COUNTERVALUE_ITEM_DOUBLE));
                    if (item.FmtValue.CStatus != ERROR_SUCCESS)
                    {
                        continue;
                    }

                    PdhValue value = new PdhValue();
                    value.Name = Marshal.PtrToStringUni(item.szName);
                    value.Value = item.FmtValue.DoubleValue;
                    values.Add(value);
                }
            }
            finally
            {
                Marshal.FreeHGlobal(buffer);
            }

            return values;
        }

        private static ulong ToUInt64(FILETIME time)
        {
            return ((ulong)time.dwHighDateTime << 32) | time.dwLowDateTime;
        }

        public struct CpuTimes
        {
            public ulong Idle;
            public ulong Kernel;
            public ulong User;
        }

        [StructLayout(LayoutKind.Sequential)]
        private struct FILETIME
        {
            public uint dwLowDateTime;
            public uint dwHighDateTime;
        }

        [StructLayout(LayoutKind.Sequential)]
        public struct MEMORYSTATUSEX
        {
            public uint dwLength;
            public uint dwMemoryLoad;
            public ulong ullTotalPhys;
            public ulong ullAvailPhys;
            public ulong ullTotalPageFile;
            public ulong ullAvailPageFile;
            public ulong ullTotalVirtual;
            public ulong ullAvailVirtual;
            public ulong ullAvailExtendedVirtual;
        }

        [StructLayout(LayoutKind.Sequential)]
        private struct PDH_FMT_COUNTERVALUE_DOUBLE
        {
            public uint CStatus;
            public double DoubleValue;
        }

        [StructLayout(LayoutKind.Sequential)]
        private struct PDH_FMT_COUNTERVALUE_ITEM_DOUBLE
        {
            public IntPtr szName;
            public PDH_FMT_COUNTERVALUE_DOUBLE FmtValue;
        }
    }
}
