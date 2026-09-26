package com.example.probe;

enum Color { RED, GREEN, BLUE }

public class Main {
    static int sink;
    static boolean flag;
    static byte b;

    static void takeBool(boolean x) { flag = x; sink += x ? 3 : 7; if (sink > 1000) System.out.println("x"); }
    static void takeByte(byte x) { b = x; sink += x; if (sink > 1000) System.out.println("y"); }

    static void p1(int n) {
        Color c = n > 5 ? Color.RED : Color.BLUE;
        takeBool(true);
        takeByte((byte) 1);
        System.out.println(c.name());
        takeBool(true);
        takeByte((byte) 3);
    }

    static void p2(int n) {
        Color c = n > 5 ? Color.GREEN : Color.RED;
        boolean[] arr = new boolean[] { true, n > 2 };
        System.out.println(c.name() + arr[0]);
        flag = true;
    }

    public static void main(String[] args) {
        int n = args.length;
        p1(n);
        p2(n);
        System.out.println(Color.valueOf(n > 5 ? "BLUE" : "GREEN").ordinal());
        System.out.println(sink + " " + flag + " " + b);
    }
}
