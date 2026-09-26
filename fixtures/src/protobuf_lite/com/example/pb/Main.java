package com.example.pb;

public class Main {
    public static void main(String[] args) {
        int n = args.length;
        System.out.println(new ExistenceFilter(n, 2, "u"));
        System.out.println(new LatLng(n + 0.5, 1.5));
        System.out.println(new Twin1(n, 1, 2));
        System.out.println(new Twin2(2, n, 1));
        System.out.println(new Unbundled(n, 3, 4));
    }
}
