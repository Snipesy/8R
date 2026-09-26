package com.example.reflect;

import java.lang.reflect.Field;

// Name-driven reflection, the way serializers and protobuf-style runtimes do it: field names
// come from a string table and are looked up with getDeclaredField. Keep rules preserve the
// field names (not the class names). 8R must not rename those fields.
final class Msg {
    int zze;
    String zzf;
    long zzg;

    // Names far along R8's generator sequence: provably kept, not generated.
    static final String[] FIELDS = { "zze", "zzf", "zzg" };

    Msg(int a, String b, long c) {
        zze = a;
        zzf = b;
        zzg = c;
    }
}

final class Pair {
    int b;
    int c;

    // Names R8 could have generated: only the string table ties them to reflection.
    static final String[] FIELDS = { "b", "c" };

    Pair(int x) {
        b = x;
        c = x * 2;
    }
}

final class Reader {
    static String dump(Object o, String[] names) throws Exception {
        StringBuilder sb = new StringBuilder();
        for (String n : names) {
            Field f = o.getClass().getDeclaredField(n);
            f.setAccessible(true);
            sb.append(n).append('=').append(f.get(o)).append(';');
        }
        return sb.toString();
    }
}

public class Main {
    public static void main(String[] args) throws Exception {
        int n = args.length;
        System.out.println(Reader.dump(new Msg(n, "s" + n, 7L + n), Msg.FIELDS));
        System.out.println(Reader.dump(new Pair(n + 3), Pair.FIELDS));
    }
}
