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
    default:
        Console.Error.WriteLine($"unknown command {args[0]}");
        return 2;
}
