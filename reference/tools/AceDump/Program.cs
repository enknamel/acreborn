// Golden-data generator: drives ACE.DatLoader (the reference implementation)
// and prints results in the same TSV shape `acdat manifest` produces, so the
// two can be diffed. ACE is AGPL; this tool only *calls* it and lives under
// reference/, it is not part of acreborn.
//
//   dotnet run -- manifest <path.dat>
using System.Security.Cryptography;
using ACE.DatLoader;

if (args.Length < 2) {
    Console.Error.WriteLine("usage: AceDump manifest <file.dat>");
    return 2;
}
switch (args[0]) {
    case "manifest": {
        var db = new DatDatabase(args[1], keepOpen: true);
        Console.WriteLine("id\toffset\tsize\titeration\tsha256");
        foreach (var kv in db.AllFiles.OrderBy(k => k.Key)) {
            var f = kv.Value;
            var bytes = db.GetReaderForFile(f.ObjectId)!.Buffer;
            var hash = Convert.ToHexString(SHA256.HashData(bytes)).ToLowerInvariant();
            Console.WriteLine($"{f.ObjectId:X8}\t{f.FileOffset}\t{f.FileSize}\t{f.Iteration}\t{hash}");
        }
        return 0;
    }
    case "isaac": {
        // Print the first N keys of ACE's ISAAC for a seed: golden vectors for ac-net.
        var seed = Convert.ToUInt32(args[1], 16);
        var n = args.Length > 2 ? int.Parse(args[2]) : 8;
        var isaac = new ACE.Common.Cryptography.ISAAC(BitConverter.GetBytes(seed));
        for (var i = 0; i < n; i++) Console.WriteLine($"{isaac.Next():x8}");
        return 0;
    }
    case "hash32": {
        var bytes = Convert.FromHexString(args[1]);
        Console.WriteLine($"{ACE.Common.Cryptography.Hash32.Calculate(bytes, bytes.Length):x8}");
        return 0;
    }
    default:
        Console.Error.WriteLine($"unknown command {args[0]}");
        return 2;
}
